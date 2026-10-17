# cinefit-harmony

Send IR commands through a Logitech Harmony Hub over its local network API. The
Hub blasts IR at the devices it is set up for, which is how cinefit presses
buttons on a player with no usable network control of its own (the UB820 reports
state over the network but refuses key presses; see `cinefit-ub820`). The same
Hub can drive anything it knows, the projector and receiver included.

The Hub's local API needs no authentication.

## Protocol

Two transports on port 8088: a plain HTTP POST (`setup.account?getProvisionInfo`)
returns the Hub's remote id, and a WebSocket carries the rest as JSON, the
configuration and `holdAction` to press a key. The crate hand-rolls a minimal
WebSocket client (`ws`) rather than take a dependency for it.

The protocol knowledge follows the open-source local clients
[aioharmony](https://github.com/ehendrix23/aioharmony) and
[pyharmony](https://github.com/iandday/pyharmony), both permissively licensed.
Neither is a dependency, and neither is shipped with cinefit; they are used on
the development rig to check this against.

A command's **label is not always what to send**: the config gives each button a
label and, inside its action, the real IR command string. The OK button is
labelled `Select` but its command is `OK`. `Device::command_for` resolves either.

## Example

    cargo run -p cinefit-harmony --example harmony -- <hub-ip> list [filter]
    cargo run -p cinefit-harmony --example harmony -- <hub-ip> press <device-id> <command>

`list` reads only; `press` fires IR.
