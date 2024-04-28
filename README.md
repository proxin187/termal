# Termal - A hackable terminal emulator with unicode support

> [!WARNING]
> Termal is still in very early development so feel free to report bugs.

## About
Termal is not supposed to be the fastest terminal emulator nor the best, termal is supposed to be your own terminal emulator.

## Features
- [x] Full vt10x support
- [x] C0 control codes
- [x] Custom CSI parser
- [x] utf-8 support
- [x] copy/paste

## Installation
Termal is installed from source with `build.sh`.

### Prequesites
In order to build termal you will need to have the rust toolchain installed and available to `build.sh`.

On arch-based distros the rust toolchain can be installed with the [rust](https://archlinux.org/packages/extra/x86_64/rust/) package.

## Configuration
Termal looks for a configuration file at `$HOME/.config/termal/config.toml`.

> [!WARNING]
> The default configuration assumes you have the Iosevka Nerd Font installed.

The default configuration is as following.
```
######################
#    Termal Config   #
######################

tab_max = 400
scrollback = 400


######################
#  Colors and looks  #
######################

# IMPORTANT: make sure to replace $HOME with your home path
bell = "$HOME/.config/termal/pluh.wav"

# xft font syntax: https://keithp.com/keithp/talks/xtc2001/xft.pdf
font = "Iosevka Nerd Font Mono:style=Regular"

foreground = "d7-e0-da"
background = "0d-16-17"

colors = [
    "0a-10-11", # black
    "e7-4b-4b", # red
    "5e-c5-87", # green
    "de-b2-6a", # brown
    "65-9b-db", # blue
    "c1-67-d9", # magneta
    "5f-d1-d5", # cyan
    "d7-e0-da", # white
]
```

## Common Issues
If you get the following error message it's most likely caused by a invalid path for the bell inside your configuration.
`[+] failed to create terminal: No such file or directory (os error 2)`

## Todos
- [ ] fix visual disturbances as a result of dirty xft rendering

## License
Termal is licensed under the MIT license.


