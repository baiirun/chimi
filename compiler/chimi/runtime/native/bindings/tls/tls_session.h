/* The TLS session engine: mbedTLS over two memory buffers. The caller moves
   ciphertext between the buffers and the real transport, so nothing here
   waits on a descriptor. */
#ifndef TLS_SESSION_H
#define TLS_SESSION_H

#include <stddef.h>

#include "mbedtls/ssl.h"
#include "mbedtls/x509_crt.h"
#include "mbedtls/pk.h"

/* The failure codes the Chimi wrapper maps to `TlsError`. */
enum {
  TLS_UNTRUSTED = 1,
  TLS_EXPIRED = 2,
  TLS_WRONG_HOST = 3,
  TLS_UNACCEPTABLE = 4,
  TLS_HANDSHAKE = 5,
  TLS_INVALID = 6,
  TLS_CLOSED = 7,
  TLS_NO_ROOTS = 8,
};

/* What `tls_receive` found. */
enum {
  TLS_DATA = 0,       /* plaintext was written to the buffer */
  TLS_NEEDS_INPUT = 1, /* feed more ciphertext, then call again */
  TLS_PEER_CLOSED = 2, /* the peer sent close_notify */
};

typedef struct {
  unsigned char *bytes;
  size_t length;
  size_t start; /* bytes before `start` are consumed */
  size_t capacity;
} TlsBuffer;

typedef struct {
  mbedtls_ssl_context ssl;
  mbedtls_ssl_config config;
  mbedtls_x509_crt roots;
  mbedtls_x509_crt certificate;
  mbedtls_pk_context key;
  TlsBuffer incoming; /* ciphertext fed in, not yet read by mbedTLS */
  TlsBuffer outgoing; /* ciphertext mbedTLS wrote, not yet taken */
  int failure;        /* a TLS_* code once the session failed; 0 before */
  char message[256];
} TlsSession;

/* Each returns NULL and fills `code` and `message` on failure. A client
   with NULL `roots` trusts the system's roots (`tls_system_roots`). */
TlsSession *tls_client_session(const unsigned char *roots, size_t roots_length,
                               const char *host, int *code, char *message, size_t size);
TlsSession *tls_server_session(const unsigned char *certificate, size_t certificate_length,
                               const unsigned char *key, size_t key_length, int *code,
                               char *message, size_t size);
void tls_free_session(TlsSession *session);
/* How many sessions are made and not yet freed. */
long tls_live_sessions(void);

/* Returns 1 when the handshake is complete, 0 when it needs input, or -1 on
   failure (`session->failure`). */
int tls_handshake(TlsSession *session);
/* Appends ciphertext from the transport. Returns -1 when out of memory. */
int tls_feed(TlsSession *session, const unsigned char *bytes, size_t length);
/* The ciphertext waiting to be sent; `tls_took` drops it once taken. */
const unsigned char *tls_pending(const TlsSession *session, size_t *length);
void tls_took(TlsSession *session);
/* Encrypts all of `bytes` into the outgoing buffer. 0, or -1 on failure. */
int tls_send(TlsSession *session, const unsigned char *bytes, size_t length);
/* Decrypts up to `size` bytes into `buffer`. Returns TLS_DATA (with
   `*length`), TLS_NEEDS_INPUT, TLS_PEER_CLOSED, or -1 on failure. */
int tls_receive(TlsSession *session, unsigned char *buffer, size_t size, size_t *length);
/* Queues close_notify. */
void tls_close_notify(TlsSession *session);
/* Loads the system's roots into `chain`: the file `SSL_CERT_FILE` names when
   it is set, else the first bundle found at the usual paths. Returns 0, or
   -1 when none loads. */
int tls_system_roots(mbedtls_x509_crt *chain);
/* Parses PEM roots; returns how many, or -1 (filling `message`). */
int tls_check_roots(const unsigned char *pem, size_t length, char *message, size_t size);

#endif
