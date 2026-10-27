# Building on Windows

KanbAIn is developed in the devcontainer (Linux) but ships as a Windows Tauri
desktop app. There are two ways to get a Windows build to test.

## Option A: let CI build it (easiest)

The `Windows build` workflow (`.github/workflows/windows-build.yml`) runs on
`windows-latest` for every push to `main` and `kanbain/**` branches, and it can
also be started by hand (`workflow_dispatch`).

1. Push the branch: `git push origin kanbain/phase-0a`
2. On GitHub, open **Actions → Windows build → the run → Artifacts**.
3. Download `windows-installers`. It contains the `.msi` and the NSIS setup
   `.exe`; install either one.

## Option B: build locally on Windows

These steps match what the workflow does.

### 1. Prerequisites

| Tool | Why | Install |
| --- | --- | --- |
| Git for Windows (incl. Git Bash) | clone + bash for the `package.json` scripts | `winget install Git.Git` |
| Visual Studio 2022 Build Tools, workload **Desktop development with C++** | MSVC linker + Windows SDK | `winget install Microsoft.VisualStudio.2022.BuildTools`, then pick the workload |
| rustup | Rust; the pinned nightly from `rust-toolchain.toml` installs itself on the first `cargo` call | `winget install Rustlang.Rustup` |
| LLVM / clang | `bindgen` (for `libsqlite3-sys`) needs `libclang` | `winget install LLVM.LLVM` |
| CMake + NASM | native build of `aws-lc-sys` (TLS) | `winget install Kitware.CMake NASM.NASM` |
| Node.js 22 | frontend | `winget install OpenJS.NodeJS.LTS` |
| pnpm 10.13.1 | package manager (pinned via `packageManager`) | `corepack enable` (admin shell), then `corepack install` in the repo |
| Tauri CLI 2 | builds the installer | `cargo install tauri-cli --version "^2" --locked` |

The `windows-latest` runner already has LLVM, CMake and NASM. On your own
machine you install them yourself. Open a **new** terminal afterwards so `PATH`
picks them up.

### 2. Clone

Clone onto a native Windows drive (e.g. `C:\dev`), **not** the `\\wsl$\...`
share. Building from the share is very slow, and file watching doesn't work there.

```powershell
git config --global core.longpaths true
git clone -b kanbain/phase-0a https://github.com/Baconiser/kanbain.git C:\dev\kanbain
cd C:\dev\kanbain
```

### 3. Build the installer

```powershell
pnpm install --frozen-lockfile
cd packages\local-web; pnpm run build; cd ..\..
cd crates\tauri-app; cargo tauri build; cd ..\..
```

The first Rust build takes a while. Output:

- `target\release\bundle\msi\*.msi`
- `target\release\bundle\nsis\*.exe`
- `target\release\kanbain-tauri.exe`, the app itself, which you can run without
  installing.

### 4. Dev mode on Windows (optional)

The `pnpm run dev` / `tauri:dev` scripts use bash syntax (`export`, `$(...)`),
which `cmd.exe` can't run. Point pnpm at Git Bash (this writes to `.npmrc`;
don't commit that change):

```powershell
pnpm config set script-shell "C:\Program Files\Git\bin\bash.exe" --location project
cargo install cargo-watch --locked
pnpm run tauri:dev     # desktop window
# or: pnpm run dev     # browser at http://localhost:3000
```

## Troubleshooting

- **`Unable to find libclang`**: install LLVM, then
  `setx LIBCLANG_PATH "C:\Program Files\LLVM\bin"` and open a new terminal.
- **`aws-lc-sys` / CMake errors**: check that `cmake` and `nasm` are on `PATH`.
- **`link.exe` not found / wrong linker**: the C++ workload of the VS Build Tools
  is missing, or Git's `link` is shadowing MSVC. Build from a "Developer
  PowerShell for VS 2022".
- **`'export' is not recognized`**: the pnpm `script-shell` isn't set (step 4).
- **Path too long**: `git config --global core.longpaths true`, and enable Win32
  long paths (`LongPathsEnabled`).
- **Slow builds**: exclude `C:\dev\kanbain\target` and `%USERPROFILE%\.cargo`
  from Defender real-time scanning.
