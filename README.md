# Bonsai.cpp Manager

Windows app (Tauri 2 + React + Rust) to install and manage the
**llama.cpp backend from the [PrismML-Eng](https://github.com/PrismML-Eng/llama.cpp) fork**
(`prism` branch, Bonsai 2 models):

- **Backend**: downloads prebuilt binaries from GitHub Releases
  (CPU / CUDA / Vulkan / HIP, auto-detected), never builds from source.
  Multiple versions side by side in `bin/<tag>/<flavor>/`, persisted default
  (`bin/default.txt`), install stops a running server first, delete refused
  while running, switching default restarts the server on the new binary.
- **Model router (default)**: the server starts with **no model loaded**;
  each OpenAI client picks the model per request via the `model` field, and
  llama-server loads it on first use. Every model gets a short **alias**
  (its name in the API) and its own parameters — context size, GPU layers,
  sampling — while shared defaults live in the `[*]` section. Inactive
  models are unloaded (`--sleep-idle-seconds`) and the resident count is
  capped (`--models-max`, LRU). The preset file is generated and validated
  against `--help`, so a parameter the installed binary does not know is
  refused before launch instead of breaking the server.
- **Dynamic configuration**: flags fetched at launch via
  `llama-server --help` and parsed (nothing hardcoded). **Router** mode
  (registry + shared defaults), **Simple** mode (Bonsai 2 preset) and
  **Expert** mode (whole `--help` by section, search, "modified" badge).
- **Models**: local catalog + Hugging Face downloads
  (default `prism-ml/Ternary-Bonsai-2-27B-gguf`, `PQ2_0` recommended,
  `Q2_0_g64` universal, optional HF token), progress + resume.
- **Server**: `llama-server` supervision (rebuilt command line,
  `reasoning_effort` preset, forced `--metrics`, logs, `/health`).
- **Statistics**: live prefill / decode t/s, per-poll averages, active slots,
  shared KV-cache usage, per-request history (memory only, with a **Modèle**
  column in router mode), optional
  **tray icon tooltip** reflection (plain icon, text only).
- **Interface**: French and English, light/dark following the Windows theme.
- **Background + tray**: close = hide to tray (server keeps running),
  Show / Start-Stop / Open web UI / Quit menu, single instance,
  optional Windows autostart (minimized).

## Prerequisites (Windows)

- Stable Rust (`winget install Rustlang.Rustup`)
- VS C++ Build Tools (`Microsoft.VisualStudio.2022.BuildTools`
  + `VCTools` workload)
- Node 24 via `nvm` + WebView2 Runtime (already on Windows 10/11)

## Development

```powershell
npm install
npm run tauri dev
```

## Installer build (NSIS, perMachine — run as admin)

```powershell
npm run tauri build
```

## App data

`%APPDATA%/com.bonsai.manager/` (bundle identifier): `bin/<tag>/<flavor>/`
(+ `bin/default.txt` for the default backend), `models/bonsai2-gguf/`,
`logs/llama-server.log`, UI presets in `localStorage`.

## Tests

```powershell
cd src-tauri
cargo test   # --help parser (tolerant snapshot)
```

## Release

Push a tag `vX.Y.Z` (matching `version` in `tauri.conf.json` +
`package.json` + `Cargo.toml`): the `Release` workflow builds the NSIS
installer on `windows-latest` and publishes it on the Releases page.

```powershell
git tag v0.7.0
git push origin v0.7.0
```

## License

MIT — see [LICENSE](LICENSE).
