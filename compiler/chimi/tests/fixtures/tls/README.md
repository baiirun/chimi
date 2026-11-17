# TLS test fixtures

Throwaway certificates for `tests/tls.rs`. Nothing trusts them outside these tests, and the keys guard nothing.

- `ca.pem` is the fixture authority, "Chimi Test CA" (P-256, valid for 100 years).
- `server.pem` and `server.key` are a leaf for `localhost`, signed by `ca.pem`.
- `expired.pem` is the same leaf and key, valid only during the year 2000.
- `other-ca.pem` and `other.key` are an unrelated self-signed authority. Its key does not match `server.pem`.

They were made with OpenSSL 3.0:

```sh
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout ca.key -out ca.pem \
  -days 36500 -subj "/CN=Chimi Test CA" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign"
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout server.key -out server.csr \
  -subj "/CN=localhost"
printf "subjectAltName=DNS:localhost\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n" > ext.cnf
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out server.pem \
  -days 36500 -extfile ext.cnf
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout other.key -out other-ca.pem \
  -days 36500 -subj "/CN=Other CA"
# expired.pem: `openssl ca` with -startdate 20000101000000Z -enddate 20010101000000Z
# and the same extensions as ext.cnf.
```

`ca.key` was not kept, so new leaves need a new authority.
