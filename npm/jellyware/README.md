# jellyware

Small live apps that agents and people shape while they run.

```bash
npm install -g jellyware    # installs the `jelly` command, no Rust needed
jelly ls                    # starts the daemon: http://127.0.0.1:7878
```

This package holds a launcher. The `jelly` binary for your platform comes from
one of its optional dependencies: macOS and Linux on arm64 and x64, and Windows
on x64. On another platform, build from source.

Docs and source: https://github.com/Wirasm/jelly
