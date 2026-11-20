# Security

## Reporting a vulnerability

Please do not open a public issue for a security problem. Report it privately through GitHub:
the repository's **Security** tab, **Report a vulnerability** (GitHub's private vulnerability
reporting). Say what is affected (the desktop app or the Android client, and the version that
Settings > About shows), how to reproduce it, and what it lets someone do.

You get an answer once the report has been read. A fix goes into the next release, and the
advisory is published with it, crediting you unless you would rather not be named.

## What counts

The apps handle things that must stay private: a Navidrome server's address, user name and
token, the Last.fm and ListenBrainz keys (sealed on disk), and the user's music files, which
only the tag editor may write. A way to read those secrets, to get them into a log or Copy
diagnostics, to make the app write to music files without tag editing turned on, or to make it
reach the network for a service the user has not set up is a vulnerability. So is a file (a track, a playlist, a cover, a theme or a preset) that makes
the app run code or write outside its own folders.

Only the latest release is supported.
