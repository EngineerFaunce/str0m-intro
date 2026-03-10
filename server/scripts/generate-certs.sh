#!/usr/bin/env bash

die() {
  echo "$1" 1>&2
  exit $2
}

# Determine the script's directory
script_dir="$(cd "$(dirname "${0}")" && pwd)" ||
  die "Couldn't determine the script's running directory, which probably matters, bailing out" 1

# Check for openssl
if [ ! command -v openssl &> /dev/null ]; then
  die "Error: OpenSSL is not installed. Please install OpenSSL to continue." 1
fi

certs_dir="$script_dir/../self_signed_certs"
key_file="$certs_dir/key.pem"
cert_file="$certs_dir/cert.pem"
config_file="$(mktemp)"

# Generate a temp OpenSSL config with SANs
cat > "$config_file" <<EOF
[req]
default_bits       = 2048
prompt             = no
default_md         = sha256
distinguished_name = dn
x509_extensions    = v3_req

[dn]
CN = localhost

[v3_req]
subjectAltName = @alt_names

[alt_names]
DNS.1 = localhost
IP.1 = 127.0.0.1
EOF

# Generate private key
openssl genpkey -algorithm RSA -out "$key_file" -pkeyopt rsa_keygen_bits:2048 || die "Key generation failed" 1

# Generate self-signed cert with SANs
openssl req -new -x509 -key "$key_file" -out "$cert_file" -days 365 -config "$config_file" -extensions v3_req || die "Cert generation failed" 1

rm -f "$config_file"

echo "✅ Self-signed certificate and key generated:"
echo " - $cert_file"
echo " - $key_file"

