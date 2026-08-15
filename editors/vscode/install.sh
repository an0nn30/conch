#!/bin/bash
# Build and install the TermLab Plugin Support extension for VS Code.
set -e

cd "$(dirname "$0")"
npm install --silent
npx tsc -p ./
npx --yes @vscode/vsce package
code --install-extension termlab-lua-*.vsix
echo "TermLab Plugin Support extension installed. Restart VS Code to activate."
