use kmo_sync::storage::RemoteStorage;
use kmo_sync::storage::s3::{S3Config, S3Storage};
use std::env;

fn r2_config_with_prefix(prefix: String) -> S3Config {
    S3Config {
        endpoint: env::var("KMO_S3_ENDPOINT").expect("KMO_S3_ENDPOINT is required"),
        bucket: env::var("KMO_S3_BUCKET").expect("KMO_S3_BUCKET is required"),
        access_key: env::var("KMO_S3_ACCESS_KEY").expect("KMO_S3_ACCESS_KEY is required"),
        secret_key: env::var("KMO_S3_SECRET_KEY").expect("KMO_S3_SECRET_KEY is required"),
        region: env::var("KMO_S3_REGION").unwrap_or_else(|_| "auto".to_string()),
        root_prefix: prefix,
        path_style: true,
        allow_http: false,
    }
}

#[tokio::test]
#[ignore = "requires R2/S3 — read-only bucket dump"]
async fn r2_dump_buckets_tree() {
    // Dump top-level "books" tree (and a few other top-level prefixes if any).
    let config = r2_config();
    let storage = S3Storage::new(config).unwrap();

    // Empty prefix lists EVERYTHING at the bucket root (after root_prefix).
    // Then specific prefixes for the yuewei/ layout.
    let probes = [
        ("", "ROOT (all objects in bucket)"),
        ("Reeden/book_progress", "Reeden/book_progress/"),
        ("Reeden/books", "Reeden/books/"),
    ];
    for (p, desc) in &probes {
        let items = if p.is_empty() {
            storage.list_prefix("").await.unwrap_or_default()
        } else {
            storage.list_prefix(p).await.unwrap_or_default()
        };
        println!("\n=== {} [{}] ({} entries) ===", desc, p, items.len());
        let mut items = items;
        items.sort_by(|a, b| a.path.cmp(&b.path));
        for it in items {
            println!("  {} ({} bytes)", it.path, it.size);
        }
    }

    // Dump the contents of every book_progress JSON + a couple of covers/indexes
    // samples so we can match kmo-sync's model to Reeden's actual schema.
    let bp_items = storage.list_prefix("Reeden/book_progress").await.unwrap();
    println!(
        "\n=== book_progress JSON contents ({} files) ===",
        bp_items.len()
    );
    for it in &bp_items {
        let raw = storage.read_object(&it.path).await.unwrap_or_default();
        let txt = String::from_utf8_lossy(&raw);
        println!("\n-- {} --", it.path);
        println!("{}", txt);
    }
    for probe in ["Reeden/indexes", "Reeden/metadata"] {
        match storage.read_object(probe).await {
            Ok(raw) => {
                let txt = String::from_utf8_lossy(&raw);
                let preview = if txt.len() > 800 {
                    format!("{}…[+{} chars]", &txt[..800], txt.len() - 800)
                } else {
                    txt.to_string()
                };
                println!("\n== {} ({} bytes) ==\n{}", probe, raw.len(), preview);
            }
            Err(e) => println!("\n== {} ERR: {} ==", probe, e),
        }
    }
    // covers is binary (zip-style archive); print first 64 hex bytes only.
    if let Ok(raw) = storage.read_object("Reeden/covers").await {
        let n = raw.len().min(64);
        let hex: String = raw[..n].iter().map(|b| format!("{:02x}", b)).collect();
        println!(
            "\n== Reeden/covers ({} bytes) ==\nfirst {} bytes hex: {}",
            raw.len(),
            n,
            hex
        );
    }
}

#[tokio::test]
#[ignore = "requires R2/S3 — deletes kmo_sync_r2_phone_pad_phone_* test objects"]
async fn r2_cleanup_phone_pad_phone_test_prefixes() {
    let storage = S3Storage::new(r2_config()).unwrap();
    let mut items = storage
        .list_prefix("kmo_sync_r2_phone_pad_phone_")
        .await
        .unwrap();
    if items.is_empty() {
        items = storage
            .list_prefix("")
            .await
            .unwrap()
            .into_iter()
            .filter(|item| item.path.starts_with("kmo_sync_r2_phone_pad_phone_"))
            .collect();
    }
    for item in &items {
        storage.remove(&item.path).await.unwrap();
    }
    println!(
        "removed {} kmo_sync_r2_phone_pad_phone_* test objects",
        items.len()
    );
}

fn r2_config() -> S3Config {
    r2_config_with_prefix(String::new())
}
