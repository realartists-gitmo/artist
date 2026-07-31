// ---- (1) impl regrouping: `impl Trait for Foo` lifts Trait into Foo.bases.
pub trait Greeter {
    fn hello(&self) -> String;
}

pub struct Person {
    pub name: String,
}

impl Person {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

impl Greeter for Person {
    fn hello(&self) -> String {
        format!("hello, {}", self.name)
    }
}

// ---- (2) extern "C" foreign mod: declared as Namespace with fn/static children.
extern "C" {
    pub fn libc_strlen(s: *const u8) -> usize;
    pub static LIBC_ERRNO: i32;
}

// ---- (3) macro_rules! definition; #[macro_export] makes it public.
#[macro_export]
macro_rules! shout {
    ($s:expr) => { $s.to_uppercase() };
}

macro_rules! private_helper {
    () => { 42 };
}

// ---- (4) tuple struct + unit struct: numeric fields / no body.
pub struct Pair(pub u8, pub u8);

pub struct Marker;

// ---- (5) trait associated types and consts.
pub trait Storage {
    type Key;
    const VERSION: u32;
    fn get(&self, k: &Self::Key) -> Option<String>;
}
