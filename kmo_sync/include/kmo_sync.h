#pragma once
#ifndef KMO_SYNC_H
#define KMO_SYNC_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct KmoSyncHandle kmo_sync_t;
typedef void (*event_callback_t)(int event_type, const char *json_data, void *user_data);

kmo_sync_t *kmo_sync_create(
    const char *storage_config_json,
    const char *encryption_config_json,
    const char *device_id,
    const char *local_cache_dir,
    event_callback_t callback,
    void *user_data
);

void kmo_sync_destroy(kmo_sync_t *sync);
int kmo_sync_all(kmo_sync_t *sync, int mode);
int kmo_sync_single_meta(kmo_sync_t *sync, const char *book_hash, const char *meta_id);
int kmo_sync_book(kmo_sync_t *sync, const char *book_hash);
int kmo_sync_put_local_book(kmo_sync_t *sync, const char *book_hash, const char *local_file_path);
int kmo_sync_put_local_meta_json(kmo_sync_t *sync, const char *meta_json);
int kmo_sync_resolve_meta_file_conflict(
    kmo_sync_t *sync,
    const char *meta_id,
    const char *chosen_version
);
int kmo_sync_mark_meta_item_deleted(
    kmo_sync_t *sync,
    const char *meta_id,
    const char *item_type,
    const char *item_uuid
);
int kmo_sync_undo_deletion(
    kmo_sync_t *sync,
    const char *meta_id,
    const char *item_uuid
);
int kmo_sync_resolve_tombstone_revival(
    kmo_sync_t *sync,
    const char *meta_id,
    const char *item_uuid,
    const char *resolution
);
int kmo_sync_resolve_blob_conflict(
    kmo_sync_t *sync,
    const char *book_hash,
    const char *chosen_version
);
int kmo_sync_rotate_envelope_kek(
    kmo_sync_t *sync,
    const char *new_encryption_config_json
);
int kmo_sync_set_network_type(kmo_sync_t *sync, int network_type);
int kmo_sync_set_blob_byte_limit(kmo_sync_t *sync, int64_t byte_limit);
int kmo_sync_pause(kmo_sync_t *sync);
int kmo_sync_resume(kmo_sync_t *sync);
char *kmo_sync_get_local_meta(kmo_sync_t *sync, const char *meta_id);
char *kmo_sync_get_sync_state(kmo_sync_t *sync);
char *kmo_sync_last_error(kmo_sync_t *sync);
void kmo_sync_free_string(char *s);
int kmo_sync_get_version(void);

#define KMO_OK 0
#define KMO_ERR_NETWORK 1
#define KMO_ERR_STORAGE 2
#define KMO_ERR_CRYPTO 3
#define KMO_ERR_CONFLICT 4
#define KMO_ERR_INVALID_ARG 5
#define KMO_ERR_INTERNAL 6
#define KMO_ERR_VERSION_MISMATCH 11

#define KMO_NETWORK_WIFI 0
#define KMO_NETWORK_CELLULAR 1
#define KMO_NETWORK_UNKNOWN 2

#ifdef __cplusplus
}
#endif

#endif
