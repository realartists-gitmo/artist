# Artist WEF vendor

- Upstream: `https://github.com/longbridge/wef`
- Commit: `034489786368e4d817d024223164a48b71afee2c`
- Local patch: `crates/webview/Cargo.toml` points at Artist's GPUI and
  gpui-component dependency graph; inherited lint declarations are removed so
  the crate can be consumed from Artist's workspace.
- CEF is not stored here. Run `scripts/fetch-cef.sh` and export the printed
  `CEF_ROOT` before building `artist-gpui --features embedded-canvas`.

The CEF archive version and SHA-1 are pinned in the fetch script.
