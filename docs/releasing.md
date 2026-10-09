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
| Windows Authenticode | CLI `.exe`, the desktop app, its installers and uninstaller | variable `WINDOWS_SIGN_COMMAND` + provider secrets | Waiting for a certificate |
| macOS Developer ID + notarisation | CLI binaries, `LoupeCam.app`, `.dmg` | `APPLE_*` secrets | Waiting for an Apple Developer account |

Until OS signing is configured, those steps are skipped and releases still work. Windows
SmartScreen and macOS Gatekeeper warn on first run.

The update signature is what the built-in updaters trust: they refuse anything not
signed with this key. **Back up `~/.tauri/loupecam.key` and
`~/.tauri/loupecam.key.password`.** Losing them means no existing install can ever be
updated again.

Verify a download:

```sh
sha256sum -c SHA256SUMS --ignore-missing
gh attestation verify loupecam-x86_64-unknown-linux-musl -R cinderblock/loupecam
```

## Windows: Authenticode

Any signing service works. Set the repository **variable** `WINDOWS_SIGN_COMMAND` to its
command line, with `%1` where the file goes, and add the service's credentials as
secrets. The workflow passes `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET` and
`AZURE_TENANT_ID` through. For another provider, add its secrets to the
`Authenticode` / `Build, sign, upload` steps.

### Recommended: Azure Artifact Signing (formerly Trusted Signing)

About US$10/month, no hardware token, and certificates are trusted by SmartScreen.

1. In the Azure portal, create an **Artifact Signing account** (pick a region, e.g.
   West US 2 → endpoint `https://wus2.codesigning.azure.net`).
2. Under the account, complete **Identity validation** (Individual is available in
   some countries; Organization needs a registered business). This takes a few days.
3. Create a **Certificate profile** (Public Trust) using the validated identity.
4. Create an **App registration** in Microsoft Entra ID, add a client secret, and grant
   it the **Artifact Signing Certificate Profile Signer** role on the account.
5. In GitHub (Settings → Secrets and variables → Actions):
   - secrets `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_TENANT_ID`
   - variable `WINDOWS_SIGN_COMMAND` =
     `artifact-signing-cli -e https://wus2.codesigning.azure.net -a <account> -c <profile> -d LoupeCam %1`

```sh
gh secret set AZURE_CLIENT_ID -R cinderblock/loupecam
gh secret set AZURE_CLIENT_SECRET -R cinderblock/loupecam
gh secret set AZURE_TENANT_ID -R cinderblock/loupecam
gh variable set WINDOWS_SIGN_COMMAND -R cinderblock/loupecam --body 'artifact-signing-cli -e https://wus2.codesigning.azure.net -a ACCOUNT -c PROFILE -d LoupeCam %1'
```

The workflow installs `artifact-signing-cli` and puts `signtool` on PATH by itself.
The wiring (Tauri signing the app, NSIS plugins, uninstaller and installer through the
command) has been tested with a stand-in command.

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
