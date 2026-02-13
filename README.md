# remipn

[![CI](https://github.com/rhslack/remipn/actions/workflows/ci.yml/badge.svg)](https://github.com/rhslack/remipn/actions/workflows/ci.yml)

**Remipn** is a modern, lightweight, cross-platform VPN manager written in Rust. It offers an interactive Terminal User Interface (TUI) and a powerful Command Line Interface (CLI) to manage your VPN connections with ease.

## Features

- 🖥️ **Interactive TUI**: Manage your VPN profiles with an intuitive terminal interface based on `ratatui`.
- ⌨️ **CLI Support**: Quick commands to connect, disconnect, and check VPN status.
- 🔄 **Smart Connection Management**: Automatically handles switching between different VPNs, ensuring only one is active at a time.
- ⏳ **Real-time Feedback**: Connection status monitoring with an automatic retry mechanism and polling.
- 📁 **Profile Import**: Supports importing profiles from XML files (including support for Azure VPN Client on macOS).
- 🔍 **Search and Filters**: Quickly find your profiles by name, category, or alias.
- 📂 **Cross-Platform**: Support for Windows (`rasdial`), Linux (`nmcli`), and macOS (`scutil`).

## Installation

Make sure you have [Rust](https://www.rust-lang.org/) installed on your system.

```bash
# Clone the repository
git clone https://github.com/yourusername/remipn.git
cd remipn

# Build and install
cargo install --path .
```

## Usage

### TUI Interface

Simply run `remipn` without arguments to launch the interactive interface:

```bash
remipn
```

**Main Shortcuts:**
- `Enter`: Connect/Disconnect the selected profile
- `n`: Add a new profile
- `e`: Edit the selected profile
- `a`: Quick alias edit for the selected profile
- `x`: Delete the selected profile
- `i`: Import profiles from XML
- `/`: Search through profiles
- `l`: Show/Hide logs
- `s`: Change sorting
- `q`: Quit

### CLI Interface

You can also use `remipn` directly from the command line:

```bash
# List all profiles
remipn list

# Connect to a profile (use name or alias)
remipn connect "ProfileName"

# Disconnect the active VPN
remipn disconnect

# Check status
remipn status
```

## Configuration

Configurations are saved in `~/.config/remipn/config.toml`.  
XML files for automatic import at startup can be placed in `~/.config/remipn/imports/`.

## License

This project is distributed under the MIT License. See the [LICENSE](LICENSE) file for details.

## Contributing

Contributions are welcome! Feel free to open issues or pull requests.
