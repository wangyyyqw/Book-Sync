use crate::{Result, SyncError};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_GRACE_PERIOD_DAYS: u32 = 90;
const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TombstoneItemType {
    Bookmark,
    Highlight,
    Note,
}

impl TombstoneItemType {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "bookmark" => Ok(Self::Bookmark),
            "highlight" => Ok(Self::Highlight),
            "note" => Ok(Self::Note),
            other => Err(SyncError::InvalidArg(format!(
                "invalid tombstone item_type: {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tombstone {
    pub uuid: String,
    pub item_type: TombstoneItemType,
    pub deleted_at_logical_ts: i64,
    pub deleted_at_wall_ts: i64,
    pub deleted_by_device: String,
    pub grace_period_days: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_item_json: Option<String>,
}

impl Tombstone {
    pub fn new(
        uuid: String,
        item_type: TombstoneItemType,
        logical_ts: i64,
        device_id: String,
    ) -> Self {
        Self {
            uuid,
            item_type,
            deleted_at_logical_ts: logical_ts,
            deleted_at_wall_ts: now_millis(),
            deleted_by_device: device_id,
            grace_period_days: DEFAULT_GRACE_PERIOD_DAYS,
            deleted_item_json: None,
        }
    }

    pub fn is_expired(&self, now_ms: i64) -> bool {
        now_ms
            > self.deleted_at_wall_ts + i64::from(self.grace_period_days).saturating_mul(DAY_MILLIS)
    }
}

/// 记录一次"撤销删除"操作，用于跨设备传播 revival 意图。
/// 当 revived_at_logical_ts > 对应 tombstone 的 deleted_at_logical_ts 时，
/// 该 tombstone 在过滤时不生效（条目被保留）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revival {
    pub uuid: String,
    pub item_type: TombstoneItemType,
    pub revived_at_logical_ts: i64,
    pub revived_at_wall_ts: i64,
    pub revived_by_device: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TombstoneSet {
    pub tombstones: Vec<Tombstone>,
    #[serde(default)]
    pub revivals: Vec<Revival>,
    pub last_modified_ts: i64,
}

impl TombstoneSet {
    pub fn mark_deleted(&mut self, tombstone: Tombstone) {
        if let Some(existing) = self
            .tombstones
            .iter_mut()
            .find(|item| item.uuid == tombstone.uuid && item.item_type == tombstone.item_type)
        {
            *existing = tombstone;
        } else {
            self.tombstones.push(tombstone);
        }
        self.last_modified_ts = now_millis();
    }

    /// 记录一次 revival 操作，使对应 tombstone 在过滤时不生效。
    pub fn add_revival(&mut self, revival: Revival) {
        if let Some(existing) = self
            .revivals
            .iter_mut()
            .find(|r| r.uuid == revival.uuid && r.item_type == revival.item_type)
        {
            if revival.revived_at_logical_ts >= existing.revived_at_logical_ts {
                *existing = revival;
            }
        } else {
            self.revivals.push(revival);
        }
        self.last_modified_ts = now_millis();
    }

    /// 判断某个 tombstone 是否已被 revival 撤销。
    pub fn is_revived(&self, tombstone: &Tombstone) -> bool {
        self.revivals.iter().any(|r| {
            r.uuid == tombstone.uuid
                && r.item_type == tombstone.item_type
                && r.revived_at_logical_ts > tombstone.deleted_at_logical_ts
        })
    }

    pub fn revive(&mut self, uuid: &str) -> bool {
        let initial_len = self.tombstones.len();
        self.tombstones.retain(|item| item.uuid != uuid);
        let changed = self.tombstones.len() != initial_len;
        if changed {
            self.last_modified_ts = now_millis();
        }
        changed
    }

    pub fn merge(&mut self, other: TombstoneSet) {
        for tombstone in other.tombstones {
            let replace = self
                .tombstones
                .iter()
                .position(|item| {
                    item.uuid == tombstone.uuid && item.item_type == tombstone.item_type
                })
                .filter(|index| {
                    tombstone.deleted_at_logical_ts >= self.tombstones[*index].deleted_at_logical_ts
                });
            if let Some(index) = replace {
                self.tombstones[index] = tombstone;
            } else if !self
                .tombstones
                .iter()
                .any(|item| item.uuid == tombstone.uuid && item.item_type == tombstone.item_type)
            {
                self.tombstones.push(tombstone);
            }
        }
        // 合并 revivals：同 uuid+item_type 取 revived_at_logical_ts 大者
        for revival in other.revivals {
            if let Some(existing) = self
                .revivals
                .iter_mut()
                .find(|r| r.uuid == revival.uuid && r.item_type == revival.item_type)
            {
                if revival.revived_at_logical_ts >= existing.revived_at_logical_ts {
                    *existing = revival;
                }
            } else {
                self.revivals.push(revival);
            }
        }
        self.last_modified_ts = self.last_modified_ts.max(other.last_modified_ts);
    }

    pub fn gc_expired(&mut self) -> usize {
        let now = now_millis();
        let initial_len = self.tombstones.len();
        // 收集被 GC 的 uuid，用于同步清理对应 revivals
        let expired_uuids: Vec<String> = self
            .tombstones
            .iter()
            .filter(|item| item.is_expired(now))
            .map(|item| item.uuid.clone())
            .collect();
        self.tombstones.retain(|item| !item.is_expired(now));
        // tombstone 过期后，对应的 revival 也没有意义了
        self.revivals.retain(|r| !expired_uuids.contains(&r.uuid));
        let removed = initial_len - self.tombstones.len();
        if removed > 0 {
            self.last_modified_ts = now;
        }
        removed
    }

    pub fn len(&self) -> usize {
        self.tombstones.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tombstones.is_empty() && self.revivals.is_empty()
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tombstone_set_roundtrips_and_revives() {
        let mut set = TombstoneSet::default();
        set.mark_deleted(Tombstone::new(
            "highlight-1".to_string(),
            TombstoneItemType::Highlight,
            7,
            "device-a".to_string(),
        ));

        let decoded = TombstoneSet::decode(&set.encode().unwrap()).unwrap();
        assert_eq!(decoded.len(), 1);

        assert!(set.revive("highlight-1"));
        assert!(set.is_empty());
    }
}
