# W3BS-SIGN-1: Publisher signing with SSH keys

**Status:** Community draft, implemented on w3bs.org  
**Date:** October 19, 2026

Publishers sign each release of a package listed on w3bs.org/packages with an ordinary SSH key. The signature uses OpenSSH's standard signature format (SSHSIG, `ssh-keygen -Y sign`), the same format git uses for SSH-signed commits. No new key type, tool or certificate authority is needed: any publisher with OpenSSH 8.2 or newer can sign, and anyone can verify with `ssh-keygen -Y verify`.

This sits on top of the registry's existing signatures. The registry (TUF, `/repo`) proves the catalog came from w3bs.org; the publisher signature proves the release came from the holder of a key bound to the package's domain. The registry can list a release but cannot forge one.

## 1 Keys

Accepted public key types, in OpenSSH `authorized_keys` form:

| Type | Notes |
| --- | --- |
| `ssh-ed25519` | Recommended |
| `sk-ssh-ed25519@openssh.com` | Hardware key (FIDO2); recommended for teams |
| `ecdsa-sha2-nistp256` | Accepted |
| `sk-ecdsa-sha2-nistp256@openssh.com` | Hardware key; accepted |

RSA and DSA keys are refused. A key is identified by its OpenSSH fingerprint, `SHA256:<unpadded base64 of sha256(key blob)>`, exactly as `ssh-keygen -lf` prints it.

## 2 Publisher namespace and domain proof

A publisher namespace is a DNS host name: the part of a package identity before the first `/` (`pairux.com` in `pairux.com/pairux`). A W3BS account may sign for a namespace once it proves both:

1. **Key possession:** the account registers the public key with a signature over the account challenge (§4), made with namespace `w3bs-key@w3bs.org`.
2. **Domain control**, by either:
   - `https://<namespace>/.well-known/w3bs-publisher.txt` served with HTTP 200 and containing the line `w3bs-publisher=<fingerprint>`; or
   - a DNS TXT record at `_w3bs.<namespace>` containing `w3bs-publisher=<fingerprint>`.

The server checks domain control when the key is added and again on every release submission. A namespace may have several keys; removing the line or record stops new releases from that key.

## 3 Release statement

A release statement is a JSON object. The signed bytes are its RFC 8785 (JCS) canonical form.

```json
{
  "type": "w3bs-release/1",
  "package": "pairux.com/pairux",
  "version": "0.17.0",
  "name": "pairux",
  "description": "Screen sharing app whose pairux command brings AI agents into a live PairUX session.",
  "homepage": "https://pairux.com",
  "repository": "https://github.com/profullstack/pairux.com",
  "license": "MIT",
  "commands": ["pairux"],
  "install": "curl -fsSL https://pairux.com/install.sh | sh",
  "installer": { "url": "https://pairux.com/install.sh", "sha256": "<hex>" },
  "artifacts": [
    { "target": "linux-x86_64", "url": "https://…/pairux.AppImage", "sha256": "<hex>" }
  ],
  "created_at": "2026-10-19T20:00:00Z"
}
```

Required: `type`, `package`, `version` (semver), `name`, `description`, `install` (a `curl -fsSL https://… | sh` line whose URL equals `installer.url`), `installer.url`, `installer.sha256`, `created_at`. `artifacts` is optional; each entry needs `target`, an `https` `url` and `sha256`.

## 4 Signatures

```sh
ssh-keygen -Y sign -n w3bs-release@w3bs.org -f ~/.ssh/id_ed25519 release.jcs.json
# writes release.jcs.json.sig (-----BEGIN SSH SIGNATURE-----)
```

| Purpose | SSHSIG namespace | Signed message |
| --- | --- | --- |
| Release | `w3bs-release@w3bs.org` | JCS bytes of the release statement |
| Key registration | `w3bs-key@w3bs.org` | `w3bs-key:<account username>:<challenge>` (UTF-8) |

The hash algorithm is `sha512` or `sha256`. A signature made with another namespace (for example a git commit signature) never verifies as a release.

Anyone can check a release offline:

```sh
echo "pairux.com namespaces=\"w3bs-release@w3bs.org\" ssh-ed25519 AAAA…" > allowed_signers
ssh-keygen -Y verify -f allowed_signers -I pairux.com -n w3bs-release@w3bs.org \
  -s release.jcs.json.sig < release.jcs.json
```

## 5 Submission

`POST /api/publish/releases` with the session or a `w3bs login` bearer:

```json
{ "statement": { … }, "signature": "-----BEGIN SSH SIGNATURE-----\n…" }
```

The server accepts the release only when all hold:

1. The signature verifies, with namespace `w3bs-release@w3bs.org`, over the JCS bytes of `statement`.
2. The signing key is registered to the caller's account for the package's namespace, is not revoked, and the domain proof (§2) still lists it.
3. The package namespace is not owned by another account. The first verified account owns it; further maintainers are added by the owner.
4. The version has not been published before. Releases are immutable.
5. The server fetches `installer.url` and its sha256 equals `installer.sha256`.
6. The package is not a reserved catalog identity owned by a different publisher in `catalog/catalog.json`.

Accepted releases appear on w3bs.org/packages with the signing fingerprint and are served at `GET /api/publish/releases/<namespace>/<name>` (all versions) and `GET /api/publish/catalog.json` (the latest release of every package, the input to the registry's signed catalog).

## 6 Installation

A client installing a signed release:

1. Takes the statement, signature and key from the registry's signed catalog.
2. Verifies the SSHSIG over the JCS bytes with namespace `w3bs-release@w3bs.org`.
3. Pins the namespace's key fingerprints on first install (trust on first use, on top of the registry signature). A later release signed by an unpinned key is refused unless the user accepts it explicitly.
4. Downloads the installer, checks its sha256 against the statement, and only then runs it. A mismatch aborts before anything executes.

## 7 Revocation

The account owner revokes a key on w3bs.org. Releases signed by a revoked key stay visible but are marked revoked, and clients refuse to install them. Removing the key from the domain proof stops new releases but does not revoke old ones.

## 8 Security notes

- Namespaces separate release signatures from git, key-registration and any other SSHSIG use.
- Hardware (`sk-`) keys require user presence for each signature; the server records the flags and counter but does not require them.
- The key's private half never leaves the publisher's machine; w3bs.org stores only public keys.
