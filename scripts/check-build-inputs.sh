#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
output=$(mktemp -d)
trap 'rm -rf "$output"' EXIT HUP INT TERM

# Use Docker's actual ignore rules and COPY set without downloading a base image.
docker build --target inputs \
    --output "type=local,dest=$output" "$root"
for file in docs/sdk-login.md docs/sdk-http.md proto/descriptors.pb proto/NOTICE.md LICENSE Cargo.lock config.example.toml; do
    test -f "$output/$file"
done
test ! -e "$output/secrets"
test ! -e "$output/target"
cargo check --locked --offline --manifest-path "$output/Cargo.toml" \
    --target-dir "$root/target/build-inputs" --workspace
