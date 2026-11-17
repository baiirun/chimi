# TLS test fixtures

Throwaway certificates for `tests/tls.rs`. Nothing trusts them outside these tests, and the keys guard nothing. Every certificate is P-256. The valid ones run from 2020 to 2120, so a test machine with a wrong clock still passes.

- `ca.pem` is the fixture authority, "Chimi Test CA".
- `server.pem` and `server.key` are a leaf for `localhost`, signed by `ca.pem`.
- `expired.pem` is the same leaf and key, valid only during the year 2000.
- `future.pem` is the same leaf and key, valid only from 2100 to 2110.
- `other-ca.pem` and `other.key` are an unrelated self-signed authority. Its key does not match `server.pem`.
- `stranger-expired.pem` is the `localhost` leaf signed by `other-ca.pem` and valid only during 2000. It is untrusted and expired at once.

They were made with OpenSSL 3.0's `openssl ca`, which sets exact dates. The config gives the authority `basicConstraints=critical,CA:TRUE` and `keyUsage=critical,keyCertSign,cRLSign`. It gives each leaf `subjectAltName=DNS:localhost`, `basicConstraints=CA:FALSE` and `extendedKeyUsage=serverAuth`.

```sh
openssl ca -batch -config ca.cnf -selfsign -keyfile ca.key -in ca.csr -out ca.pem \
  -startdate 20200101000000Z -enddate 21200101000000Z -extensions root -notext
openssl ca -batch -config ca.cnf -cert ca.pem -keyfile ca.key -in server.csr -out server.pem \
  -startdate 20200101000000Z -enddate 21200101000000Z -extensions leaf -notext
```

`ca.key` was not kept, so new leaves need a new authority.
