#!/bin/sh
# Generates a local CA and a server certificate for localhost (TEST ONLY) into ./certs.
# mailbackup always validates certificates, so the CA must be trusted by the OS (see README.md).
set -eu
cd "$(dirname "$0")"
mkdir -p certs
cd certs
openssl req -x509 -newkey rsa:2048 -nodes -days 30 -keyout ca.key -out ca.crt \
  -subj "/CN=mailbackup test CA" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign"
openssl req -newkey rsa:2048 -nodes -keyout server.key -out server.csr -subj "/CN=localhost"
printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n' > server.ext
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 30 \
  -extfile server.ext -out server.crt
rm -f server.csr server.ext ca.srl
chmod 644 server.key
echo "Generated certs/ca.crt (trust this) and certs/server.{crt,key}."
