import SwiftUI

struct ContentView: View {
    @State private var status = "Book Sync iOS sample ready"

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Book Sync")
                .font(.title)
                .bold()
            Text(status)
                .font(.body)
            Button("Push iOS Meta") {
                status = pushIosMeta()
            }
            Button("Pull Shared Meta") {
                status = pullSharedMeta()
            }
            Button("Show Sync State") {
                status = showSyncState()
            }
            Button("Resolve First Conflict") {
                status = resolveFirstConflict()
            }
        }
        .padding(24)
    }

    private func pushIosMeta() -> String {
        withSync { handle in
            let putCode = sampleMetaJson(metaId: "shared-meta", progress: 0.84, logicalTs: 20)
                .withCString { meta in
                    kmo_sync_put_local_meta_json(handle, meta)
                }
            guard putCode == KMO_OK else {
                return lastErrorMessage(handle: handle, code: putCode)
            }
            let code = kmo_sync_all(handle, 1)
            guard code == KMO_OK else {
                return lastErrorMessage(handle: handle, code: code)
            }
            return "iOS pushed shared-meta progress 0.84"
        }
    }

    private func pullSharedMeta() -> String {
        withSync { handle in
            let code = kmo_sync_all(handle, 2)
            guard code == KMO_OK else {
                return lastErrorMessage(handle: handle, code: code)
            }
            let metaPointer = "shared-meta".withCString { metaId in
                kmo_sync_get_local_meta(handle, metaId)
            }
            let meta = metaPointer.map { String(cString: $0) } ?? "null"
            if let metaPointer {
                kmo_sync_free_string(metaPointer)
            }
            return "Pulled shared-meta:\n\(meta)"
        }
    }

    private func showSyncState() -> String {
        withSync { handle in
            syncState(handle: handle) ?? "Sync state unavailable"
        }
    }

    private func resolveFirstConflict() -> String {
        withSync { handle in
            guard let state = syncState(handle: handle),
                  let data = state.data(using: .utf8),
                  let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let conflicts = json["conflicts"] as? [[String: Any]],
                  let conflict = conflicts.first else {
                return "No pending conflicts"
            }

            let kind = conflict["conflict_kind"] as? String
            if kind == "meta_file", let metaId = conflict["meta_id"] as? String {
                let code = metaId.withCString { meta in
                    "remote".withCString { chosen in
                        kmo_sync_resolve_meta_file_conflict(handle, meta, chosen)
                    }
                }
                guard code == KMO_OK else {
                    return lastErrorMessage(handle: handle, code: code)
                }
                return "Resolved meta conflict for \(metaId) with remote"
            }

            if kind == "tombstone_revival",
               let metaId = conflict["meta_id"] as? String,
               let itemUuid = conflict["item_uuid"] as? String {
                let code = metaId.withCString { meta in
                    itemUuid.withCString { item in
                        "restore".withCString { resolution in
                            kmo_sync_resolve_tombstone_revival(handle, meta, item, resolution)
                        }
                    }
                }
                guard code == KMO_OK else {
                    return lastErrorMessage(handle: handle, code: code)
                }
                return "Restored tombstone conflict \(itemUuid)"
            }

            if kind == "blob_file", let bookHash = conflict["book_hash"] as? String {
                let code = bookHash.withCString { hash in
                    "remote".withCString { chosen in
                        kmo_sync_resolve_blob_conflict(handle, hash, chosen)
                    }
                }
                guard code == KMO_OK else {
                    return lastErrorMessage(handle: handle, code: code)
                }
                return "Resolved blob conflict for \(bookHash) with remote"
            }

            return "Unsupported conflict: \(kind ?? "unknown")"
        }
    }

    private func withSync(_ action: (OpaquePointer) -> String) -> String {
        let cacheDir = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("kmo-sync", isDirectory: true)
            .path

        let handle = storageConfigJson().withCString { storage in
            "{\"type\":\"none\"}".withCString { encryption in
                "ios-sample".withCString { device in
                    cacheDir.withCString { cache in
                        kmo_sync_create(storage, encryption, device, cache, nil, nil)
                    }
                }
            }
        }

        guard let handle else {
            return "Book Sync create failed"
        }
        defer {
            kmo_sync_destroy(handle)
        }
        return action(handle)
    }

    private func storageConfigJson() -> String {
        guard let url = Bundle.main.url(forResource: "kmo_sync_sample_config", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let text = String(data: data, encoding: .utf8) else {
            return "{\"type\":\"memory\"}"
        }
        return text
    }

    private func syncState(handle: OpaquePointer) -> String? {
        let statePointer = kmo_sync_get_sync_state(handle)
        let state = statePointer.map { String(cString: $0) }
        if let statePointer {
            kmo_sync_free_string(statePointer)
        }
        return state
    }

    private func sampleMetaJson(metaId: String, progress: Double, logicalTs: Int) -> String {
        """
        {
          "meta_id":"\(metaId)",
          "book_hash":"shared-sample-book",
          "modified_ts":\(logicalTs),
          "device_id":"ios-sample",
          "progress":{
            "cfi_position":"epubcfi(/6/2)",
            "progress_percent":\(progress),
            "chapter_id":"chapter-1"
          },
          "bookmarks":[],
          "highlights":[],
          "notes":[],
          "wall_clock_ts":\(logicalTs),
          "logical_ts":\(logicalTs),
          "origin_device_id":"ios-sample",
          "edit_history":[]
        }
        """
    }

    private func lastErrorMessage(handle: OpaquePointer, code: Int32) -> String {
        let messagePointer = kmo_sync_last_error(handle)
        let message = messagePointer.map { String(cString: $0) } ?? "unknown error"
        if let messagePointer {
            kmo_sync_free_string(messagePointer)
        }
        return "Book Sync failed: \(code) \(message)"
    }
}
