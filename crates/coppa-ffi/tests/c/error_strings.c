/*
 * COP-139 scenario 2: a failed call through the C interface explains itself.
 *
 * Makes 9 invalid or edge-case calls, prints each result with
 * coppa_last_error(), and checks the text. Exits 1 at the first mismatch.
 * Then prints and checks the four version numbers (COP-178).
 * Builds as C11 and as C++17 with -Wall -Wextra -Wpedantic -Werror; CI runs
 * both (.github/workflows/ci.yml).
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "coppa.h"

static int passed = 0;

/* Print "<label> = <shown>: <last error>" and check the error text.
 * expect == NULL means "no error is set". */
static int check(const char *label, const char *shown, const char *expect) {
  const char *err = coppa_last_error();
  printf("%s = %s: %s\n", label, shown, err ? err : "(no error)");
  if (expect == NULL ? err != NULL : (err == NULL || strstr(err, expect) == NULL)) {
    fprintf(stderr, "FAIL: %s: expected %s\n", label, expect ? expect : "no error");
    return 0;
  }
  passed++;
  return 1;
}

static int check_rc(const char *label, int32_t rc, int32_t want, const char *expect) {
  char shown[16];
  snprintf(shown, sizeof shown, "%d", (int)rc);
  if (rc != want) {
    fprintf(stderr, "FAIL: %s returned %d, expected %d\n", label, (int)rc, (int)want);
    return 0;
  }
  return check(label, shown, expect);
}

static int check_null(const char *label, const void *p, const char *expect) {
  if (p != NULL) {
    fprintf(stderr, "FAIL: %s returned a handle, expected NULL\n", label);
    return 0;
  }
  return check(label, "NULL", expect);
}

int main(void) {
  float *samples = NULL;
  uintptr_t len = 0;
  static uint8_t big[4096];
  coppa_config_t cfg;
  coppa_engine_t *engine;
  coppa_engine_t *h;
  const char *text;

  if (!check_rc("coppa_encode(NULL handle)", coppa_encode(NULL, "hi", &samples, &len),
                COPPA_ERR_NULL_ARG, "coppa_encode: handle is null"))
    return 1;

  engine = coppa_engine_create();
  if (engine == NULL) {
    fprintf(stderr, "FAIL: coppa_engine_create returned NULL\n");
    return 1;
  }
  if (!check_rc("coppa_encode_bytes(4096 B)",
                coppa_encode_bytes(engine, big, sizeof big, &samples, &len), COPPA_ERR_CODEC,
                "coppa_encode_bytes: payload too large for this speed level (max 56 bytes)"))
    return 1;
  if (!check_rc("coppa_engine_set_speed_level(77)", coppa_engine_set_speed_level(engine, 77),
                COPPA_ERR_INVALID_PARAM, "coppa_engine_set_speed_level: Invalid speed level: 77"))
    return 1;
  coppa_engine_destroy(&engine);

  memset(&cfg, 0, sizeof cfg);
  cfg.profile = "NOPE";
  h = coppa_engine_new_with(&cfg);
  if (!check_null("coppa_engine_new_with(profile=NOPE)", h,
                  "coppa_engine_new_with: unknown profile \"NOPE\" (expected HF_ROBUST, "
                  "HF_STANDARD, VHF_FAST or EMERGENCY)"))
    return 1;

  memset(&cfg, 0, sizeof cfg);
  cfg.profile = "HF_ROBUST";
  cfg.speed_level = 9;
  h = coppa_engine_new_with(&cfg);
  if (!check_null("coppa_engine_new_with(HF_ROBUST, level 9)", h,
                  "coppa_engine_new_with: speed level 9 is not available on an HF profile"))
    return 1;

  memset(&cfg, 0, sizeof cfg);
  cfg.sample_rate = 8000;
  h = coppa_engine_new_with(&cfg);
  if (!check_null("coppa_engine_new_with(sample_rate=8000)", h,
                  "coppa_engine_new_with: sample_rate 8000 is not supported: coppa runs at "
                  "48000 Hz (pass 0 or 48000)"))
    return 1;

  /* The v1 streaming quartet is deprecated (COPPA_DEPRECATED); these two
   * checks still use it to show NULL-with-no-error versus NULL-with-error. */
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wdeprecated-declarations"
  {
    CoppaStreamHandle *stream = coppa_start_stream();
    char *msg = coppa_get_decoded(stream);
    if (!check_null("coppa_get_decoded(empty stream)", msg, NULL))
      return 1;
    msg = coppa_get_decoded(NULL);
    if (!check_null("coppa_get_decoded(NULL)", msg, "coppa_get_decoded: handle is null"))
      return 1;
    coppa_stop_stream(stream);
  }
#pragma GCC diagnostic pop

  text = coppa_strerror(COPPA_ERR_POISONED);
  printf("coppa_strerror(-4) = %s\n", text);
  if (text == NULL || strstr(text, "destroy it and create a new one") == NULL) {
    fprintf(stderr, "FAIL: coppa_strerror(-4)\n");
    return 1;
  }
  passed++;

  /* COP-178: four separate version numbers, none needing a handle. The two
   * scalar accessors need ABI 1.1, so check the loaded library first. */
  {
    const char *err_before = coppa_last_error();
    uint32_t abi = coppa_abi_version();
    const char *crate = coppa_version();
    int has_v11 = (abi >> 16) == 1u && (abi & 0xFFFFu) >= 1u;
    printf("versions: crate=%s air_protocol=%u host_api=%u abi=%u.%u\n", crate ? crate : "(null)",
           has_v11 ? (unsigned)coppa_air_protocol_version() : 0u,
           has_v11 ? (unsigned)coppa_host_api_version() : 0u, (unsigned)(abi >> 16),
           (unsigned)(abi & 0xFFFFu));
    if ((abi >> 16) != COPPA_ABI_VERSION_MAJOR || (abi & 0xFFFFu) < COPPA_ABI_VERSION_MINOR ||
        !has_v11) {
      fprintf(stderr, "FAIL: loaded ABI %u.%u is older than this header's %u.%u\n",
              (unsigned)(abi >> 16), (unsigned)(abi & 0xFFFFu), (unsigned)COPPA_ABI_VERSION_MAJOR,
              (unsigned)COPPA_ABI_VERSION_MINOR);
      return 1;
    }
    if (crate == NULL || crate[0] == '\0' || strchr(crate, '.') == NULL) {
      fprintf(stderr, "FAIL: coppa_version() is not a semver string\n");
      return 1;
    }
    /* Only "set": a later library may carry a newer air or host-API version
     * without a C ABI break, and this file runs as the old-caller smoke. */
    if (coppa_air_protocol_version() == 0 || coppa_host_api_version() == 0) {
      fprintf(stderr, "FAIL: air protocol and host API versions must be nonzero\n");
      return 1;
    }
    if (coppa_last_error() != err_before) {
      fprintf(stderr, "FAIL: version accessors must not touch coppa_last_error\n");
      return 1;
    }
  }

  printf("all %d checks passed\n", passed);
  return passed == 9 ? 0 : 1;
}
