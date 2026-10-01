# KeeSpot

Spotlight-style search for your KeePassXC database. Press a hotkey, type, hit Enter, and the password is in your clipboard.

macOS only. Early version.

https://github.com/user-attachments/assets/f1ea284c-af20-4df6-94c7-b7efd5cd0097

## Features

- Global hotkey (`⌘⇧Space`) opens a search bar
- Search across title, username, URL and group
- Copy password, username or TOTP code
- Clipboard is cleared after 15 seconds
- Locks itself after 5 minutes without use
- Follows the system light and dark mode
- Menu bar only, no Dock icon

## Requirements

- macOS
- [KeePassXC](https://keepassxc.org) (`keepassxc-cli` is found in the Homebrew and app bundle locations, or in your `PATH`)
- [Node.js](https://nodejs.org) and [Rust](https://rustup.rs) to build from source

## Run

```bash
npm install
npm run tauri dev
```

Build an app bundle:

```bash
npm run tauri build
```

The build is not code-signed or notarized. macOS will refuse to open it with a plain double-click. Right-click the app, choose "Open", then confirm. You only need to do this once.

## Usage

1. Start the app. A search icon appears in the menu bar.
2. Press `⌘⇧Space`.
3. On first launch, choose your `.kdbx` file and enter the master password.
4. Type to search.

| Key     | Action         |
| ------- | -------------- |
| `↵`     | Copy password  |
| `⌘U`    | Copy username  |
| `⌘T`    | Copy TOTP      |
| `↑` `↓` | Move selection |
| `Esc`   | Close          |

The menu bar icon lets you lock, switch database or quit.

## Config

Stored in `~/Library/Application Support/com.rehan.keespot/config.json`.

```json
{
  "db_path": "/path/to/database.kdbx",
  "clipboard_timeout_secs": 15,
  "auto_lock_secs": 300
}
```

Set `auto_lock_secs` to `0` to disable auto-lock.

## How it works

KeeSpot calls `keepassxc-cli` for everything. It does not parse the `.kdbx` file itself. The master password stays in memory while unlocked and is wiped when the app locks.

## Known limitations

- Entries with the same title in the same group can only be copied from the first one. `keepassxc-cli` finds entries by path only. Rename them in KeePassXC.
- Every copy takes about a second, because `keepassxc-cli` derives the key again each time.
- No key file or YubiKey support yet.
- No Touch ID unlock yet.

## License

[MIT](LICENSE)
