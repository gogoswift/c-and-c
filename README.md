[中文](README.zh.md) | English

# c&c — Claude & Codex Desktop Monitor

A tiny always-on-top desktop widget that shows an animated cat reflecting the real-time activity of your AI coding agents.

> No active agents? The cat sleeps. Agents running? The cat runs, plays, and stretches along with them.

---

## What it does

**c&c** sits in the corner of your screen (180×140px, transparent, frameless) and monitors:

- **Claude Code sessions** — detects whether Claude Code is actively running
- **OpenAI Codex sessions** — detects whether Codex is actively running
- **Claude usage quota** — displays your current Claude usage
- **Codex rate limits** — displays your current Codex quota

The cat sprite animation reflects agent activity:
| State | Animation |
|-------|-----------|
| No active agents | 😴 Sleeping |
| Agents working | 🏃 Running / playing / stretching (random per cat) |

Each active agent gets its own cat. Multiple agents = multiple cats.

---

## Screenshots

<table>
  <tr>
    <td><img src="assets/night.png" width="180" alt="Night"/></td>
    <td><img src="assets/dawn.png" width="180" alt="Dawn"/></td>
    <td><img src="assets/day.png" width="180" alt="Day"/></td>
    <td><img src="assets/dusk.png" width="180" alt="Dusk"/></td>
  </tr>
  <tr>
    <td align="center">Night</td>
    <td align="center">Dawn</td>
    <td align="center">Day</td>
    <td align="center">Dusk</td>
  </tr>
  <tr>
    <td><img src="assets/cc.png" width="180" alt="Usage quota"/></td>
    <td><img src="assets/setting.png" width="180" alt="Settings"/></td>
    <td><img src="assets/cat.png" width="180" alt="Menu bar"/></td>
  </tr>
  <tr>
    <td align="center">Usage quota</td>
    <td align="center">Settings</td>
    <td align="center">Menu bar</td>
  </tr>
</table>

---

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Frontend | React 18 + TypeScript + Vite |
| Desktop shell | Tauri 2 (Rust) |
| Styling | Plain CSS |
| Data | Tauri IPC commands → Rust backend |

---

## Prerequisites

- **Node.js** ≥ 18
- **Rust** ≥ 1.75 (recommended 1.90+)
- **macOS** with Xcode Command Line Tools

```bash
# Install Rust if needed
curl --proto '=https' --tlsv1.2 https://sh.rustup.rs -sSf | sh

# Install Xcode CLI tools if needed
xcode-select --install
```

---

## Getting Started

```bash
# Clone
git clone https://github.com/gogoswift/c-and-c.git
cd c-and-c

# Install frontend dependencies
npm install

# Start development (Vite + Rust hot-reload)
npx tauri dev
```

Frontend changes hot-reload instantly. Rust changes trigger automatic recompilation.

---

## Build

```bash
npx tauri build
```

Output:
- **App**: `src-tauri/target/release/bundle/macos/c&c.app`
- **DMG**: `src-tauri/target/release/bundle/dmg/c&c_0.1.0_aarch64.dmg`

> **Note:** The app is unsigned. On first launch: right-click → Open → Confirm. Or allow it in System Settings → Privacy & Security.

---

## FAQ

**First build is slow** — Rust compiles all dependencies from scratch (~1-2 min). Subsequent incremental builds take seconds.

**`cargo check` can't find tauri** — Run `npm install` in the project root first. The Tauri CLI is installed via npm.

**`tauri dev` doesn't pick up Rust changes** — It should auto-recompile on file save. If not, `Ctrl+C` and restart.

---

## Contributing

PRs and issues welcome. This is a vibe project — keep it fun.

---

## License

GPL v3 — see [LICENSE](LICENSE) for details.
