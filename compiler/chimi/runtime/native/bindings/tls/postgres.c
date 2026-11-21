/* std/postgres's natives. They live in the tls package because SCRAM needs
   the hash, MAC and key derivation of the mbedTLS this package already
   links, and a second copy of the crypto would be worse than an odd home.

   - native_scram(password, salt, iterations, auth_message) computes the
     client proof and the expected server signature of SCRAM-SHA-256
     (RFC 5802 section 3, RFC 7677).
   - native_random(count) gives `count` bytes from the PSA generator, for the
     client nonce.
   - A gate is one connection's shared state, kept here because it must
     outlive the task that opened the connection: which call holds the
     connection (a token, so a late release cannot touch a later holder),
     whether a call was cut off part way, which leaves the protocol state
     unknown, the transaction status and depth, and the bytes read past the
     last reply (std/postgres.chimi's `exclusive`). */
#define _POSIX_C_SOURCE 200809L
#include "chimi_binding.h"

#include <mbedtls/platform_util.h>
#include <psa/crypto.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>

enum { SHA256_BYTES = 32, MAX_RANDOM = 1024 };

static int crypto_ready(void) { return psa_crypto_init() == PSA_SUCCESS; }

/* HMAC-SHA-256 of `data` under `key`. */
static int hmac(const unsigned char *key, size_t key_length, const unsigned char *data,
                size_t data_length, unsigned char out[SHA256_BYTES]) {
  psa_key_attributes_t attributes = PSA_KEY_ATTRIBUTES_INIT;
  psa_set_key_type(&attributes, PSA_KEY_TYPE_HMAC);
  psa_set_key_usage_flags(&attributes, PSA_KEY_USAGE_SIGN_MESSAGE);
  psa_set_key_algorithm(&attributes, PSA_ALG_HMAC(PSA_ALG_SHA_256));
  psa_key_id_t id = 0;
  if (psa_import_key(&attributes, key, key_length, &id) != PSA_SUCCESS) return 0;
  size_t length = 0;
  psa_status_t status = psa_mac_compute(id, PSA_ALG_HMAC(PSA_ALG_SHA_256), data, data_length,
                                        out, SHA256_BYTES, &length);
  psa_destroy_key(id);
  return status == PSA_SUCCESS && length == SHA256_BYTES;
}

static int pbkdf2(const unsigned char *password, size_t password_length,
                  const unsigned char *salt, size_t salt_length, uint64_t iterations,
                  unsigned char out[SHA256_BYTES]) {
  psa_key_derivation_operation_t operation = PSA_KEY_DERIVATION_OPERATION_INIT;
  int ok = psa_key_derivation_setup(&operation, PSA_ALG_PBKDF2_HMAC(PSA_ALG_SHA_256)) ==
               PSA_SUCCESS &&
           psa_key_derivation_input_integer(&operation, PSA_KEY_DERIVATION_INPUT_COST,
                                            iterations) == PSA_SUCCESS &&
           psa_key_derivation_input_bytes(&operation, PSA_KEY_DERIVATION_INPUT_SALT, salt,
                                          salt_length) == PSA_SUCCESS &&
           psa_key_derivation_input_bytes(&operation, PSA_KEY_DERIVATION_INPUT_PASSWORD,
                                          password, password_length) == PSA_SUCCESS &&
           psa_key_derivation_output_bytes(&operation, out, SHA256_BYTES) == PSA_SUCCESS;
  psa_key_derivation_abort(&operation);
  return ok;
}

/* native_scram(password: Bytes, salt: Bytes, iterations: Int, auth_message: Bytes)
   -> Result<(Bytes, Bytes), (Int, Text)>: the client proof and the server
   signature. */
ChimiBindingResult *chimi_tls_postgres_scram(ChimiBindingCall *call) {
  const unsigned char *password, *salt, *message;
  size_t password_length, salt_length, message_length;
  int64_t iterations = 0;
  if (!chimi_binding_bytes(call, 0, &password, &password_length) ||
      !chimi_binding_bytes(call, 1, &salt, &salt_length) ||
      !chimi_binding_integer(call, 2, &iterations) ||
      !chimi_binding_bytes(call, 3, &message, &message_length)) {
    return chimi_binding_defect(call, "postgres: scram takes a password, salt, count and message");
  }
  static const unsigned char none[1] = {0};
  if (!password) password = none;
  if (!salt) salt = none;
  if (!message) message = none;
  /* RFC 7677 asks for at least 4096, and PostgreSQL's default is 4096. A
     server asking for far more would hold the task's thread inside this
     call, where connect_timeout cannot reach it; a million takes about a
     second. */
  if (iterations < 1 || iterations > 1000000) {
    return chimi_binding_failure(call, 1, "the server asked for an iteration count out of range");
  }
  if (!crypto_ready()) return chimi_binding_failure(call, 1, "the crypto library did not start");
  unsigned char salted[SHA256_BYTES], client_key[SHA256_BYTES], stored_key[SHA256_BYTES];
  unsigned char client_signature[SHA256_BYTES], proof[SHA256_BYTES];
  unsigned char server_key[SHA256_BYTES], server_signature[SHA256_BYTES];
  size_t stored_length = 0;
  int ok = pbkdf2(password, password_length, salt, salt_length, (uint64_t)iterations, salted) &&
           hmac(salted, SHA256_BYTES, (const unsigned char *)"Client Key", 10, client_key) &&
           psa_hash_compute(PSA_ALG_SHA_256, client_key, SHA256_BYTES, stored_key,
                            SHA256_BYTES, &stored_length) == PSA_SUCCESS &&
           hmac(stored_key, SHA256_BYTES, message, message_length, client_signature) &&
           hmac(salted, SHA256_BYTES, (const unsigned char *)"Server Key", 10, server_key) &&
           hmac(server_key, SHA256_BYTES, message, message_length, server_signature);
  if (ok) {
    for (int i = 0; i < SHA256_BYTES; i++) proof[i] = client_key[i] ^ client_signature[i];
  }
  /* Any of these with the proof sent gives the client key, which logs in. */
  mbedtls_platform_zeroize(salted, sizeof salted);
  mbedtls_platform_zeroize(client_key, sizeof client_key);
  mbedtls_platform_zeroize(stored_key, sizeof stored_key);
  mbedtls_platform_zeroize(client_signature, sizeof client_signature);
  mbedtls_platform_zeroize(server_key, sizeof server_key);
  if (!ok) {
    mbedtls_platform_zeroize(proof, sizeof proof);
    return chimi_binding_failure(call, 1, "the SCRAM computation failed");
  }
  ChimiBindingValue *items[2] = {chimi_binding_make_bytes(call, proof, SHA256_BYTES),
                                 chimi_binding_make_bytes(call, server_signature, SHA256_BYTES)};
  mbedtls_platform_zeroize(proof, sizeof proof);
  return chimi_binding_ok(call, chimi_binding_make_tuple(call, items, 2));
}

/* native_random(count: Int) -> Bytes */
ChimiBindingResult *chimi_tls_postgres_random(ChimiBindingCall *call) {
  int64_t count = 0;
  if (!chimi_binding_integer(call, 0, &count) || count < 0 || count > MAX_RANDOM) {
    return chimi_binding_defect(call, "postgres: random takes a count from 0 to 1024");
  }
  unsigned char bytes[MAX_RANDOM];
  if (!crypto_ready() || psa_generate_random(bytes, (size_t)count) != PSA_SUCCESS) {
    return chimi_binding_defect(call, "postgres: the random generator failed");
  }
  return chimi_binding_success(call, chimi_binding_make_bytes(call, bytes, (size_t)count));
}

/* A connection's gate. `holder` is the token of the call holding it (0 when
   free), `status` the last ReadyForQuery's transaction status ('I', 'T' or
   'E'), `depth` the transaction levels open, and `leftover` the bytes read
   past the last reply. */
typedef struct {
  pthread_mutex_t lock;
  int64_t holder;
  int64_t tokens;
  int broken;
  int status;
  int64_t depth;
  unsigned char *leftover;
  size_t leftover_length;
} Gate;

static void release_gate(void *state) {
  Gate *gate = state;
  pthread_mutex_destroy(&gate->lock);
  free(gate->leftover);
  free(gate);
}

static const ChimiBindingType GATE = {"PostgresGate", release_gate};

/* native_gate(status: Int, leftover: Bytes) -> PostgresGate: a connection
   that finished its startup, free. */
ChimiBindingResult *chimi_tls_postgres_gate(ChimiBindingCall *call) {
  int64_t status = 0;
  const unsigned char *bytes;
  size_t length;
  if (!chimi_binding_integer(call, 0, &status) || !chimi_binding_bytes(call, 1, &bytes, &length)) {
    return chimi_binding_defect(call, "postgres: gate takes a status and bytes");
  }
  Gate *gate = calloc(1, sizeof *gate);
  if (!gate || pthread_mutex_init(&gate->lock, NULL) != 0) {
    free(gate);
    return chimi_binding_defect(call, "postgres: out of memory");
  }
  gate->status = (int)status;
  if (length > 0) {
    gate->leftover = malloc(length);
    if (!gate->leftover) {
      release_gate(gate);
      return chimi_binding_defect(call, "postgres: out of memory");
    }
    memcpy(gate->leftover, bytes, length);
    gate->leftover_length = length;
  }
  ChimiBindingValue *handle = chimi_binding_make_handle(call, &GATE, gate);
  if (!handle) return chimi_binding_defect(call, "postgres: too many open connections");
  return chimi_binding_success(call, handle);
}

/* native_claim(gate) -> Option<(Int, Bytes)>: Some((token, leftover)) when
   the caller now holds the connection and must pass the token to
   native_settle or native_abandon after; None while another call holds it
   or once it is broken (native_status says which). */
ChimiBindingResult *chimi_tls_postgres_claim(ChimiBindingCall *call) {
  Gate *gate = chimi_binding_handle(call, 0, &GATE);
  if (!gate) return chimi_binding_success(call, chimi_binding_none(call));
  pthread_mutex_lock(&gate->lock);
  ChimiBindingValue *answer;
  if (gate->broken || gate->holder != 0) {
    answer = chimi_binding_none(call);
  } else {
    gate->holder = ++gate->tokens;
    ChimiBindingValue *items[2] = {
        chimi_binding_make_integer(call, gate->holder),
        chimi_binding_make_bytes(call, gate->leftover, gate->leftover_length)};
    answer = chimi_binding_some(call, chimi_binding_make_tuple(call, items, 2));
  }
  pthread_mutex_unlock(&gate->lock);
  return chimi_binding_success(call, answer);
}

/* native_settle(gate, token, status: Int, leftover: Bytes) -> Void: the call
   holding `token` read through ReadyForQuery, so the connection is in step
   again and free. Any other token is ignored. */
ChimiBindingResult *chimi_tls_postgres_settle(ChimiBindingCall *call) {
  Gate *gate = chimi_binding_handle(call, 0, &GATE);
  int64_t token = 0, status = 0;
  const unsigned char *bytes;
  size_t length;
  if (!chimi_binding_integer(call, 1, &token) || !chimi_binding_integer(call, 2, &status) ||
      !chimi_binding_bytes(call, 3, &bytes, &length)) {
    return chimi_binding_defect(call, "postgres: settle takes a gate, a token, a status and bytes");
  }
  if (!gate) return chimi_binding_success(call, chimi_binding_void(call));
  unsigned char *copy = NULL;
  if (length > 0) {
    copy = malloc(length);
    if (!copy) return chimi_binding_defect(call, "postgres: out of memory");
    memcpy(copy, bytes, length);
  }
  pthread_mutex_lock(&gate->lock);
  if (gate->holder == token && token != 0) {
    free(gate->leftover);
    gate->leftover = copy;
    gate->leftover_length = length;
    gate->status = (int)status;
    gate->holder = 0;
    copy = NULL;
  }
  pthread_mutex_unlock(&gate->lock);
  free(copy);
  return chimi_binding_success(call, chimi_binding_void(call));
}

/* native_abandon(gate, token) -> Void: if the call holding `token` still
   holds the connection, it did not read through ReadyForQuery, so the
   protocol state is unknown and the connection is broken for good. A
   settled call, and any other token, is left alone. */
ChimiBindingResult *chimi_tls_postgres_abandon(ChimiBindingCall *call) {
  Gate *gate = chimi_binding_handle(call, 0, &GATE);
  int64_t token = 0;
  if (!chimi_binding_integer(call, 1, &token)) {
    return chimi_binding_defect(call, "postgres: abandon takes a gate and a token");
  }
  if (gate) {
    pthread_mutex_lock(&gate->lock);
    if (gate->holder == token && token != 0) {
      gate->holder = 0;
      gate->broken = 1;
    }
    pthread_mutex_unlock(&gate->lock);
  }
  return chimi_binding_success(call, chimi_binding_void(call));
}

/* native_status(gate) -> Int: the transaction status, or -1 when broken. */
ChimiBindingResult *chimi_tls_postgres_status(ChimiBindingCall *call) {
  Gate *gate = chimi_binding_handle(call, 0, &GATE);
  int status = -1;
  if (gate) {
    pthread_mutex_lock(&gate->lock);
    status = gate->broken ? -1 : gate->status;
    pthread_mutex_unlock(&gate->lock);
  }
  return chimi_binding_success(call, chimi_binding_make_integer(call, status));
}

/* native_depth(gate) -> Int: the transaction levels open, as last set. */
ChimiBindingResult *chimi_tls_postgres_depth(ChimiBindingCall *call) {
  Gate *gate = chimi_binding_handle(call, 0, &GATE);
  int64_t depth = 0;
  if (gate) {
    pthread_mutex_lock(&gate->lock);
    depth = gate->depth;
    pthread_mutex_unlock(&gate->lock);
  }
  return chimi_binding_success(call, chimi_binding_make_integer(call, depth));
}

/* native_set_depth(gate, depth: Int) -> Void */
ChimiBindingResult *chimi_tls_postgres_set_depth(ChimiBindingCall *call) {
  Gate *gate = chimi_binding_handle(call, 0, &GATE);
  int64_t depth = 0;
  if (!chimi_binding_integer(call, 1, &depth)) {
    return chimi_binding_defect(call, "postgres: set_depth takes a gate and a depth");
  }
  if (gate) {
    pthread_mutex_lock(&gate->lock);
    gate->depth = depth;
    pthread_mutex_unlock(&gate->lock);
  }
  return chimi_binding_success(call, chimi_binding_void(call));
}
