#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "kmo_sync.h"

int main(void) {
    assert(kmo_sync_get_version() == 1);

    kmo_sync_t *bad = kmo_sync_create(
        "{",
        "{\"type\":\"none\"}",
        "device-a",
        "/tmp/kmo-sync-ffi-bad",
        NULL,
        NULL
    );
    assert(bad == NULL);

    kmo_sync_t *sync = kmo_sync_create(
        "{\"type\":\"memory\"}",
        "{\"type\":\"none\"}",
        "device-a",
        "/tmp/kmo-sync-ffi",
        NULL,
        NULL
    );
    assert(sync != NULL);
    assert(kmo_sync_all(sync, 0) == 0);
    assert(kmo_sync_set_network_type(sync, KMO_NETWORK_CELLULAR) == 0);
    assert(kmo_sync_all(sync, 0) == 0);
    assert(kmo_sync_set_network_type(sync, KMO_NETWORK_WIFI) == 0);
    assert(kmo_sync_set_blob_byte_limit(sync, -1) == 0);
    assert(kmo_sync_pause(sync) == 0);
    assert(kmo_sync_resume(sync) == 0);

    const char *meta_json =
        "{"
        "\"meta_id\":\"meta-ffi-1\","
        "\"book_hash\":\"book-ffi-1\","
        "\"modified_ts\":1,"
        "\"device_id\":\"device-a\","
        "\"progress\":{\"cfi_position\":\"epubcfi(/6/2)\",\"progress_percent\":0.5,\"chapter_id\":\"chapter-1\"},"
        "\"bookmarks\":[],"
        "\"highlights\":[{\"highlight_id\":\"highlight-ffi-1\",\"cfi_start\":\"a\",\"cfi_end\":\"b\",\"color\":\"yellow\",\"comment\":\"restore me\",\"create_ts\":1}],"
        "\"notes\":[],"
        "\"wall_clock_ts\":1,"
        "\"logical_ts\":1,"
        "\"origin_device_id\":\"device-a\","
        "\"edit_history\":[]"
        "}";
    assert(kmo_sync_put_local_meta_json(sync, meta_json) == 0);
    assert(kmo_sync_single_meta(sync, "book-ffi-1", "meta-ffi-1") == 0);
    assert(kmo_sync_mark_meta_item_deleted(sync, "meta-ffi-1", "highlight", "highlight-ffi-1") == 0);

    char *meta = kmo_sync_get_local_meta(sync, "meta-1");
    assert(meta != NULL);
    kmo_sync_free_string(meta);

    char *state = kmo_sync_get_sync_state(sync);
    assert(state != NULL);
    assert(strstr(state, "\"conflict_count\":") != NULL);
    assert(strstr(state, "\"tombstone_count\":") != NULL);
    kmo_sync_free_string(state);

    assert(kmo_sync_undo_deletion(sync, "meta-ffi-1", "highlight-ffi-1") == 0);
    assert(kmo_sync_resolve_tombstone_revival(
        sync,
        "meta-ffi-1",
        "highlight-ffi-1",
        "restore"
    ) == KMO_OK);

    char *err = kmo_sync_last_error(sync);
    assert(err != NULL);
    kmo_sync_free_string(err);

    FILE *book = fopen("/tmp/kmo-sync-ffi-book.epub", "wb");
    assert(book != NULL);
    const char *book_bytes = "ffi epub bytes";
    fwrite(book_bytes, 1, strlen(book_bytes), book);
    fclose(book);
    assert(kmo_sync_put_local_book(
        sync,
        "90d7f8abdd20e59527de5cc34458973c2bb856a04196bb397d8d3bb2cbe3e153",
        "/tmp/kmo-sync-ffi-book.epub"
    ) == 0);
    assert(kmo_sync_book(
        sync,
        "90d7f8abdd20e59527de5cc34458973c2bb856a04196bb397d8d3bb2cbe3e153"
    ) == 0);

    kmo_sync_destroy(sync);

    for (int i = 0; i < 1000; i++) {
        kmo_sync_t *loop = kmo_sync_create(
            "{\"type\":\"memory\"}",
            "{\"type\":\"none\"}",
            "device-a",
            "/tmp/kmo-sync-ffi-loop",
            NULL,
            NULL
        );
        assert(loop != NULL);
        kmo_sync_destroy(loop);
    }

    puts("ffi smoke ok");
    return 0;
}
