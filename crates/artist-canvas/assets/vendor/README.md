# Vendored canvas dependencies

These files are committed on purpose. A canvas has to open with no network and
no `npm install`, so the dependency set is compiled into the binary via
`include_dir!` — the same stance the harness takes for prompts and profiles.

Everything here is prebuilt ESM. Nothing in this directory is authored by us
except this file; `../client.js` is ours.

## Regenerating

Each file is fetched from a pinned URL. `esm.sh` serves a small re-export shim
at the friendly URL and the real bundle underneath it, so these point at the
underlying `.mjs` directly.

Two rules govern the choices below:

- **`external=react`** on anything that depends on React, so it imports the bare
  specifier `"react"` and resolves through our import map to the single copy in
  `react.js`. A second inlined React would break hooks.
- **development builds** for React, so component stacks, hook warnings, and
  readable error messages reach `canvas status`. This is what Vite serves in dev
  too, and it is why `react-dom-client.js` is ~1 MB. The dev JSX runtime and the
  production React bundle are *not* interchangeable — mixing them fails at
  runtime with `dispatcher.getOwner is not a function`.

```sh
V=crates/artist-canvas/assets/vendor

# React 19.2.8 — development, bundled
curl -sSfL -o $V/react.js \
  "https://esm.sh/react@19.2.8/es2022/react.development.bundle.mjs"
curl -sSfL -o $V/react-dom-client.js \
  "https://esm.sh/react-dom@19.2.8/X-ZXJlYWN0/es2022/client.development.bundle.mjs"
curl -sSfL -o $V/react-jsx-runtime.js \
  "https://esm.sh/react@19.2.8/X-ZXJlYWN0/es2022/jsx-runtime.development.bundle.mjs"
curl -sSfL -o $V/react-jsx-dev-runtime.js \
  "https://esm.sh/react@19.2.8/X-ZXJlYWN0/es2022/jsx-dev-runtime.development.bundle.mjs"

# React Refresh 0.18.0 — the runtime half of the oxc `refresh` transform
curl -sSfL -o $V/react-refresh-runtime.js \
  "https://esm.sh/react-refresh@0.18.0/es2022/react-refresh.development.bundle.mjs"

# uPlot 1.6.32
curl -sSfL -o $V/uplot.js  "https://esm.sh/uplot@1.6.32/es2022/uplot.bundle.mjs"
curl -sSfL -o $V/uplot.css "https://esm.sh/uplot@1.6.32/dist/uPlot.min.css"

# TanStack Table 8.21.3
curl -sSfL -o $V/tanstack-react-table.js \
  "https://esm.sh/@tanstack/react-table@8.21.3/X-ZXJlYWN0/es2022/react-table.bundle.mjs"

# Tailwind 4.3.3 — the browser JIT build, a classic script rather than a module
curl -sSfL -o $V/tailwind-browser.js \
  "https://cdn.jsdelivr.net/npm/@tailwindcss/browser@4.3.3/dist/index.global.js"
```

## After updating

`assets::tests::every_mapped_specifier_actually_ships` catches a rename, but not
a version mismatch between React and its JSX runtime — that only shows up in a
browser. Run the manual harness and click something:

```sh
cargo run -p artist-canvas --example serve -- <project-dir> <slug>
```

## Licences

React, React Refresh, and Tailwind CSS are MIT (Meta, Meta, Tailwind Labs).
uPlot is MIT (Leon Sorokin). TanStack Table is MIT (Tanner Linsley).
