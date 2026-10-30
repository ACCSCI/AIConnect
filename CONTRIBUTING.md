# Contributing

Build with Rust 1.95+ and a platform C/C++ compiler:

```sh
cargo build --locked -p aiconnect-cli
cargo test --locked --workspace
```

The website is a static React app using shadcn/ui primitives:

```sh
cd site
npm ci
npm run build
npm test
npm run test:local
npm run dev
```

Use separate local profiles for network tests. Do not commit identity keys,
database files, credentials, personal messages, or paid infrastructure defaults.
Follow the explicit grants and budgets in the Core API. Preserve the distinction
between transport delivery, execution permission, job completion, and review.

Changes should include focused verification for altered protocol, persistence,
authorization, or installation behavior. Use issues to discuss larger changes.
