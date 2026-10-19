# Security Policy

## Supported Versions

Only the latest published release receives security fixes. If you are running an
older version, upgrade before reporting an issue.

## Reporting a Vulnerability

Please **do not open a public issue** for security problems.

- Preferred: use GitHub's private vulnerability reporting via the repository's
  **Security → Advisories → Report a vulnerability** page.
- Alternative: email <paterkleomenis@protonmail.com> with the subject
  `Connected security report`.

Include, when possible:

- the affected component (desktop, Android, iOS, FFI, or the wire protocol);
- the version or commit you tested;
- reproduction steps or a proof-of-concept;
- the impact you believe the issue has.

You will receive an acknowledgement as soon as possible. Please allow time for a
fix and a coordinated release before publishing details.

## Scope

In scope:

- pairing, trust establishment, and peer-identity verification;
- the QUIC transport, framing, and file-transfer protocol;
- the signed update/verification path and release artifacts;
- the desktop, Android, and iOS clients, including the UniFFI bridge.

Out of scope:

- vulnerabilities requiring a rooted/jailbroken device or physical access to an
  unlocked device;
- denial of service caused solely by resource exhaustion on an untrusted
  network that cannot be mitigated by normal rate limiting;
- issues in third-party dependencies that are already publicly disclosed and
  tracked by Dependabot or `cargo audit`.

## Security Model Notes

- All transfers are end-to-end encrypted; devices are trusted by public-key
  fingerprint after pairing.
- Release manifests are signed with minisign, and the updater verifies the
  pinned public key before installing anything.
- Discovery is unauthenticated by design; discovered peers are not trusted until
  pairing completes.
