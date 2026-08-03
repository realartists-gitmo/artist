//! Linux local IPC using message-preserving `SOCK_SEQPACKET` sockets.

use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    ffi::CString,
    fs,
    mem::{self, MaybeUninit},
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

const MAX_PACKET: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerCredentials {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
}

pub struct SeqPacketListener {
    fd: OwnedFd,
    path: PathBuf,
}

impl SeqPacketListener {
    pub fn bind(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if path.exists() {
            bail!("socket already exists: {}", path.display());
        }
        let fd = socket()?;
        let (address, length) = socket_address(&path)?;
        syscall(unsafe {
            libc::bind(
                fd.as_raw_fd(),
                &address as *const libc::sockaddr_un as *const libc::sockaddr,
                length,
            )
        })
        .with_context(|| format!("bind {}", path.display()))?;
        syscall(unsafe { libc::listen(fd.as_raw_fd(), 16) }).context("listen")?;
        Ok(Self { fd, path })
    }

    pub fn accept(&self) -> Result<SeqPacket> {
        let fd = syscall(unsafe {
            libc::accept4(
                self.fd.as_raw_fd(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_CLOEXEC,
            )
        })?;
        Ok(SeqPacket {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        })
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> Result<()> {
        set_nonblocking(self.fd.as_raw_fd(), nonblocking)
    }
}

impl Drop for SeqPacketListener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub struct SeqPacket {
    fd: OwnedFd,
}

impl SeqPacket {
    pub fn connect(path: &Path) -> Result<Self> {
        let fd = socket()?;
        let (address, length) = socket_address(path)?;
        syscall(unsafe {
            libc::connect(
                fd.as_raw_fd(),
                &address as *const libc::sockaddr_un as *const libc::sockaddr,
                length,
            )
        })
        .with_context(|| format!("connect {}", path.display()))?;
        Ok(Self { fd })
    }

    pub fn peer_credentials(&self) -> Result<PeerCredentials> {
        let mut credentials = MaybeUninit::<libc::ucred>::uninit();
        let mut length = mem::size_of::<libc::ucred>() as libc::socklen_t;
        syscall(unsafe {
            libc::getsockopt(
                self.fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                credentials.as_mut_ptr().cast(),
                &mut length,
            )
        })?;
        let credentials = unsafe { credentials.assume_init() };
        Ok(PeerCredentials {
            pid: credentials.pid as u32,
            uid: credentials.uid,
            gid: credentials.gid,
        })
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> Result<()> {
        set_nonblocking(self.fd.as_raw_fd(), nonblocking)
    }

    pub fn send<T: Serialize>(&self, value: &T) -> Result<()> {
        self.send_with_fds(value, &[])
    }

    /// Send one atomic JSON packet with DMA-BUF or other descriptor ownership
    /// attached through `SCM_RIGHTS`.
    pub fn send_with_fds<T: Serialize>(&self, value: &T, fds: &[RawFd]) -> Result<()> {
        let bytes = serde_json::to_vec(value)?;
        if bytes.len() > MAX_PACKET {
            bail!("host packet exceeds {MAX_PACKET} bytes");
        }
        let mut iov = libc::iovec {
            iov_base: bytes.as_ptr().cast_mut().cast(),
            iov_len: bytes.len(),
        };
        let control_len = if fds.is_empty() {
            0
        } else {
            unsafe { libc::CMSG_SPACE(std::mem::size_of_val(fds) as u32) as usize }
        };
        let mut control = vec![0_u8; control_len];
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        if !fds.is_empty() {
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = control.len();
            unsafe {
                let header = libc::CMSG_FIRSTHDR(&message);
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(fds) as u32) as usize;
                std::ptr::copy_nonoverlapping(
                    fds.as_ptr().cast::<u8>(),
                    libc::CMSG_DATA(header),
                    std::mem::size_of_val(fds),
                );
            }
        }
        let written = syscall_ssize(unsafe {
            libc::sendmsg(self.fd.as_raw_fd(), &message, libc::MSG_NOSIGNAL)
        })? as usize;
        if written != bytes.len() {
            bail!("short seqpacket write: {written}/{}", bytes.len());
        }
        Ok(())
    }

    pub fn receive<T: DeserializeOwned>(&self) -> Result<T> {
        self.receive_with_fds().map(|(value, _)| value)
    }

    /// Receive a packet and take ownership of every attached descriptor.
    pub fn receive_with_fds<T: DeserializeOwned>(&self) -> Result<(T, Vec<OwnedFd>)> {
        let mut bytes = vec![0_u8; MAX_PACKET];
        let mut control =
            vec![
                0_u8;
                unsafe { libc::CMSG_SPACE((32 * std::mem::size_of::<RawFd>()) as u32) as usize }
            ];
        let mut iov = libc::iovec {
            iov_base: bytes.as_mut_ptr().cast(),
            iov_len: bytes.len(),
        };
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = control.len();
        let count = syscall_ssize(unsafe {
            libc::recvmsg(self.fd.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC)
        })? as usize;
        if count == 0 {
            bail!("host disconnected");
        }
        if message.msg_flags & libc::MSG_CTRUNC != 0 {
            bail!("host packet contained too many descriptors");
        }
        bytes.truncate(count);
        let value = serde_json::from_slice(&bytes).context("decode host packet")?;
        let mut received = Vec::new();
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(&message);
            while !header.is_null() {
                if (*header).cmsg_level == libc::SOL_SOCKET
                    && (*header).cmsg_type == libc::SCM_RIGHTS
                {
                    let data_len = (*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                    let count = data_len / std::mem::size_of::<RawFd>();
                    let fds =
                        std::slice::from_raw_parts(libc::CMSG_DATA(header).cast::<RawFd>(), count);
                    received.extend(fds.iter().map(|fd| OwnedFd::from_raw_fd(*fd)));
                }
                header = libc::CMSG_NXTHDR(&message, header);
            }
        }
        Ok((value, received))
    }
}

fn socket() -> Result<OwnedFd> {
    let fd = syscall(unsafe {
        libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0)
    })?;
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn socket_address(path: &Path) -> Result<(libc::sockaddr_un, libc::socklen_t)> {
    let bytes = path.as_os_str().as_bytes();
    let _ = CString::new(bytes).context("socket path contains NUL")?;
    let mut address = unsafe { MaybeUninit::<libc::sockaddr_un>::zeroed().assume_init() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    if bytes.len() >= address.sun_path.len() {
        bail!("socket path is too long: {}", path.display());
    }
    for (target, source) in address.sun_path.iter_mut().zip(bytes) {
        *target = *source as libc::c_char;
    }
    let length = mem::size_of_val(&address.sun_family) + bytes.len() + 1;
    Ok((address, length as libc::socklen_t))
}

fn syscall(result: libc::c_int) -> std::io::Result<libc::c_int> {
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

fn syscall_ssize(result: libc::ssize_t) -> std::io::Result<libc::ssize_t> {
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

fn set_nonblocking(fd: libc::c_int, nonblocking: bool) -> Result<()> {
    let flags = syscall(unsafe { libc::fcntl(fd, libc::F_GETFL) })?;
    let flags = if nonblocking {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };
    syscall(unsafe { libc::fcntl(fd, libc::F_SETFL, flags) })?;
    Ok(())
}
