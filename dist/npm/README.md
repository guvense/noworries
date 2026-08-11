# noworries (npm)

Prebuilt-binary distribution of [noworries](https://github.com/guvense/noworries).

```bash
npm install -g noworries
noworries install-command      # add the /noworries slash command to Claude Code
```

`postinstall` downloads the native binary for your platform (macOS arm64/x64,
Linux x64) from the matching GitHub release. Requires Docker running at use
time. For unsupported platforms, build from source:
`cargo install --git https://github.com/guvense/noworries`.
