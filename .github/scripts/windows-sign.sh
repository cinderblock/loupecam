#!/usr/bin/env bash
# Authenticode-sign files with the command in the WINDOWS_SIGN_COMMAND repository
# variable, where %1 stands for the file. Provider credentials come from secrets in the
# environment. Example (Azure Artifact Signing):
#   artifact-signing-cli -e https://wus2.codesigning.azure.net -a ACCOUNT -c PROFILE -d LoupeCam %1
#   + secrets AZURE_CLIENT_ID, AZURE_CLIENT_SECRET, AZURE_TENANT_ID
# With `setup` as the only argument, installs the provider tooling instead.
set -euo pipefail
cmd="${WINDOWS_SIGN_COMMAND:?WINDOWS_SIGN_COMMAND not set}"
if [ "${1:-}" = setup ]; then
  # signtool (Windows SDK) is installed on GitHub's Windows runners but not on PATH.
  sdk=$(ls -d "/c/Program Files (x86)/Windows Kits/10/bin/"10.*/x64 | sort -V | tail -1)
  echo "$sdk" >> "${GITHUB_PATH:-/dev/null}"
  case "$cmd" in
    artifact-signing-cli*) cargo install --locked artifact-signing-cli ;;
    trusted-signing-cli*) cargo install --locked trusted-signing-cli ;;
  esac
  exit 0
fi
# Split the template on spaces (its own arguments must not contain spaces), then put
# each file in place of %1 so file paths with spaces stay one argument.
read -r -a template <<< "$cmd"
for f in "$@"; do
  argv=()
  for a in "${template[@]}"; do argv+=("${a//%1/$f}"); done
  "${argv[@]}"
done
