# kasou (仮想)

Safe Rust bindings over Apple's **Virtualization.framework** for macOS VM
management, via `objc2-virtualization`.

Kasou is the macOS VM backend for `tatara` (workload orchestrator) and `kikai`
(K3s lifecycle). Its Linux sibling is `tateru` (libkrun); both are unified
behind the backend-neutral `maquina-engine` trait, so callers drive a VM
without knowing which is underneath.

## Usage

```toml
[dependencies]
kasou = "0.1"
```

macOS only — the crate wraps a platform framework that exists nowhere else.

## License

MIT — see [LICENSE](./LICENSE).
