#!/bin/bash

die() {
  echo "$1" 1>&2
  exit $2
}

# Determine the script's directory
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)" ||
  die "Couldn't determine the script's running directory, which probably matters, bailing out" 1

# Check for openssl
if [ ! command -v openssl &> /dev/null ]; then
  die "Error: OpenSSL is not installed. Please install OpenSSL to continue." 1
fi

certs_dir="$script_dir/../src/certs"

# Generate a private key
openssl genpkey -algorithm RSA -out $certs_dir/key.pem -pkeyopt rsa_keygen_bits:2048

# Generate a self-signed certificate
openssl req -new -x509 -key $certs_dir/key.pem -out $certs_dir/cer.pem -days 365