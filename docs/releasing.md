# Releasing and code signing

## Making a release

```sh
git tag -a v1.2.3 -m "LoupeCam 1.2.3"
git push origin v1.2.3
```

That is the whole process. The [Release workflow](../.github/workflows/release.yml)
takes the version from the tag, builds every platform, signs everything, publishes the
GitHub release, and updates the manifest that installed copies check for updates.

- `vX.Y.Z-suffix` (e.g. `v1.3.0-rc.1`) publishes a **pre-release**. Installed apps
  ignore pre-releases.
- The `version` in `Cargo.toml` does not need bumping; the tag wins. Bump it anyway when
  convenient so local builds report something sensible.
- To rebuild an existing tag with a fixed workflow: Actions → Release → *Run workflow*,
  with the tag.

## What gets signed

| Layer | Covers | Configured by | Status |
| --- | --- | --- | --- |
| Update signature (minisign) | Every artifact | secret `TAURI_SIGNING_PRIVATE_KEY` (+ `_PASSWORD`) | **Active** |
| Checksums + build provenance | Every artifact | built in (`SHA256SUMS`, GitHub attestations) | **Active** |
| Windows Authenticode | CLI `.exe`, the desktop app, its installers and uninstaller | variable `WINDOWS_SIGN_COMMAND` + provider secrets | Later (EV certificate planned) |
| macOS Developer ID + notarisation | CLI binaries, `LoupeCam.app`, `.dmg` | `APPLE_*` secrets | Not planned yet |

The update signature is what makes self-update safe, and it is all that releases need
today. OS code signing is optional polish against first-run SmartScreen/Gatekeeper
warnings. Its hooks are in the workflow but do nothing until configured.

The update signature is what the built-in updaters trust: they refuse anything not
signed with this key. **Back up `~/.tauri/loupecam.key` and
`~/.tauri/loupecam.key.password`.** Losing them means no existing install can ever be
updated again.

Verify a download:

```sh
sha256sum -c SHA256SUMS --ignore-missing
gh attestation verify loupecam-x86_64-unknown-linux-musl -R cinderblock/loupecam
```

## Windows: Authenticode (later, EV)

The plan is an EV code-signing certificate. EV keys live in an HSM, so signing goes
through the issuer's tooling (e.g. DigiCert KeyLocker with `signtool` or `smctl`). The
workflow already has a provider-agnostic hook for it: set the repository **variable**
`WINDOWS_SIGN_COMMAND` to the signing command line, with `%1` where the file goes, e.g.

```text
signtool sign /sha1 <cert-thumbprint> /tr http://timestamp.digicert.com /td sha256 /fd sha256 %1
```

and add the provider's credentials as secrets. The command then signs the CLI `.exe`, and
Tauri runs it for the desktop app, the NSIS plugins, the uninstaller and the installer
(this wiring was tested with a stand-in command). The provider's client setup (e.g.
installing KeyLocker tools and its client certificate) still needs adding to the
`windows-sign.sh setup` step, and its secrets to the signing steps' `env`.

## macOS: Developer ID and notarisation

Needs the Apple Developer Program (US$99/year).

1. In Certificates, Identifiers & Profiles, create a **Developer ID Application**
   certificate. This requires a Mac (Keychain Access → Certificate Assistant → Request
   a Certificate…). Export it with its private key as a `.p12` with a password.
2. In App Store Connect → Users and Access → Integrations → **App Store Connect API**,
   create a key with the *Developer* role. Note the **Issuer ID** and **Key ID**, and
   download the `.p8`.
3. Add secrets:

```sh
base64 -i DeveloperID.p12 | gh secret set APPLE_CERTIFICATE -R cinderblock/loupecam
gh secret set APPLE_CERTIFICATE_PASSWORD -R cinderblock/loupecam
gh secret set APPLE_SIGNING_IDENTITY -R cinderblock/loupecam   # "Developer ID Application: Name (TEAMID)"
gh secret set APPLE_API_ISSUER -R cinderblock/loupecam
gh secret set APPLE_API_KEY -R cinderblock/loupecam            # the Key ID
gh secret set APPLE_API_PRIVATE_KEY -R cinderblock/loupecam < AuthKey_XXXX.p8
```

Tauri signs and notarises the app and `.dmg`. The CLI binaries are signed with the
hardened runtime and submitted for notarisation by
[`macos-sign.sh`](../.github/scripts/macos-sign.sh). Bare executables can't be stapled;
Gatekeeper checks their notarisation online.

## Linux

Linux has no OS code-signing standard. The update signature, `SHA256SUMS` and
provenance attestations cover it.
