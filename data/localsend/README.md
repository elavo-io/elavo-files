# LocalSend receiver

Files has LocalSend (protocol v2, falling back to v1) built in. When a Files
window is open it already discovers devices and receives. To receive with no
window open, start `elavo-files --localsend-daemon` at login. If a Files
instance is already running, the second one exits right away.

- `io.elavo.Files.LocalSend.desktop`: XDG autostart entry, goes in `/etc/xdg/autostart/` or `~/.config/autostart/`
- `elavo-files-localsend.service`: systemd user unit, the alternative to the autostart entry

Settings are stored in `~/.config/elavo/localsend.conf` under the `[LocalSend]` group, with the keys Alias (optional),
Receive, SaveFolder, RequireApproval, PIN, Port, Encryption and AskFor (seconds a request waits for an answer, default 300). The TLS identity is stored in
`~/.config/elavo/localsend/`.

Ports: TCP 53317 (HTTPS API) and UDP 53317 (multicast 224.0.0.167).
Environment: `ELAVO_LOCALSEND=0` disables LocalSend and `ELAVO_LOCALSEND_TTL` sets the multicast TTL (tests use 0).

The name other devices see is the computer's name from System Settings (General → About → Name, which is
hostnamed's PrettyHostname, falling back to the static hostname). Files follows it live and announces again
when it changes. `Alias=` overrides it; an old made-up alias (an adjective and a fruit) is removed on start.
`ELAVO_HOSTNAME_BUS=session` and `ELAVO_LOCALSEND_DISCOVERY_PORT` are test hooks.
