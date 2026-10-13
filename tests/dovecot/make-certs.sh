#!/bin/sh
# Generates a local CA and a server certificate for localhost (TEST ONLY) into ./certs.
# mailbackup always validates certificates, so the CA must be trusted by the OS (see README.md).
set -eu
# Git for Windows (MSYS) would otherwise rewrite "/CN=..." into a Windows path.
export MSYS_NO_PATHCONV=1
cd "$(dirname "$0")"
mkdir -p certs
cd certs
# The CA key never outlives this script, even when a step fails.
trap 'rm -f ca.key' EXIT
# The CA may only vouch for localhost / 127.0.0.1 (name constraints), so trusting it can never
# let anyone intercept other TLS sites on this machine. Its private key is deleted at the end.
openssl req -x509 -newkey rsa:2048 -nodes -days 30 -keyout ca.key -out ca.crt \
  -subj "/CN=mailbackup test CA" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" \
  -addext "nameConstraints=critical,permitted;DNS:localhost,permitted;IP:127.0.0.1/255.255.255.255"
openssl req -newkey rsa:2048 -nodes -keyout server.key -out server.csr -subj "/CN=localhost"
printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n' > server.ext
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 30 \
  -extfile server.ext -out server.crt
rm -f server.csr server.ext ca.srl
# Nothing else is ever signed: drop the CA key so it cannot be stolen or misused.
rm -f ca.key
chmod 644 server.key
echo "Generated certs/ca.crt (trust this; limited to localhost/127.0.0.1) and certs/server.{crt,key}."
echo "ca.key was deleted. Remove the CA from the trust store when you are done (see README.md)."
