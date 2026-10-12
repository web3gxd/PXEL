# Security policy

PXEL is pre-release software. There is no stable release or support SLA yet.
Security fixes target the current development branch; published releases will
state their supported versions. Deployment gates are tracked in
[self-hosting](docs/self-hosting.md).

Report a suspected vulnerability privately with GitHub's **Report a vulnerability**
button in this repository's Security tab. Maintainers must enable private
vulnerability reporting before publication. If the button is unavailable, open
an issue requesting a private reporting channel without exploit details or data.
Do not post live credentials, workbook contents or an exploit against another
person's installation.

Include the tested commit, operating system, configuration with secrets removed,
affected endpoint and a small reproduction using a disposable instance. Specify
the access level required and the impact. Coordinate public disclosure with the
maintainer; no response-time promise is made.

Self-host operators should use a trusted HTTPS endpoint, exact allowed Origins,
a private database and file store, an unprivileged service account and protected
keyring files. The master account has full access to all instance books. Encryption
at rest does not hide content from the running server or the master account.
Development authentication must never be used for a shared installation.
