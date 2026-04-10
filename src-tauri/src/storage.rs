// Local SQLite persistence layer.
//
// Schema matches the production design in BugJuice-Claude-Code-Scope.md —
// sensors / readings / battery_sessions / sleep_sessions / app_power /
// health_snapshots / hourly_stats / daily_stats. For the prototype we
// keep everything in a single .db file at %LOCALAPPDATA%\BugJuice\bugjuice.db
// (monthly partitioning comes in Phase 2).
//
// Concurrency model per the scope doc:
//   - One dedicated writer.
//   - WAL mode guarantees readers never block the writer.
//   - Prototype uses a single Mutex<Connection> because there's no real
//     reader pool yet — good enough until Tauri wires it up.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Result as SqlResult, params};

use crate::battery::{BATTERY_UNKNOWN_CAPACITY, BatterySnapshot};

// ─── Constants ────────────────────────────────────────────────────────────────

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sensors (
    sensor_id   INTEGER PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    unit        TEXT NOT NULL,
    category    TEXT NOT NULL,
    hw_source   TEXT
);

CREATE TABLE IF NOT EXISTS readings (
    ts          INTEGER NOT NULL,
    sensor_id   INTEGER NOT NULL,
    value       REAL NOT NULL,
    session_id  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_readings_cover ON readings(sensor_id, ts, value);

CREATE TABLE IF NOT EXISTS battery_sessions (
    session_id      INTEGER PRIMARY KEY,
    started_at      INTEGER NOT NULL,
    ended_at        INTEGER,
    start_percent   REAL,
    end_percent     REAL,
    start_capacity  INTEGER,
    end_capacity    INTEGER,
    avg_drain_watts REAL,
    on_ac           INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS sleep_sessions (
    id              INTEGER PRIMARY KEY,
    sleep_at        INTEGER NOT NULL,
    wake_at         INTEGER,
    pre_capacity    INTEGER,
    post_capacity   INTEGER,
    drain_mwh       INTEGER,
    drain_percent   REAL,
    drain_rate_mw   REAL,
    drips_percent   REAL,
    sleep_type      TEXT
);

CREATE TABLE IF NOT EXISTS app_power (
    ts              INTEGER NOT NULL,
    process_name    TEXT NOT NULL,
    cpu_watts       REAL,
    gpu_watts       REAL,
    disk_watts      REAL,
    net_watts       REAL,
    total_watts     REAL
);
CREATE INDEX IF NOT EXISTS idx_app_power ON app_power(ts, process_name);

CREATE TABLE IF NOT EXISTS health_snapshots (
    ts                   INTEGER NOT NULL,
    design_capacity      INTEGER NOT NULL,
    full_charge_capacity INTEGER NOT NULL,
    cycle_count          INTEGER,
    wear_percent         REAL,
    voltage_mv           INTEGER,
    temperature_c        REAL
);

CREATE TABLE IF NOT EXISTS hourly_stats (
    sensor_id   INTEGER NOT NULL,
    hour_ts     INTEGER NOT NULL,
    min_val     REAL, max_val REAL, avg_val REAL,
    count_val   INTEGER, sum_val REAL, sum_sq REAL,
    PRIMARY KEY (sensor_id, hour_ts)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS daily_stats (
    sensor_id   INTEGER NOT NULL,
    day_ts      INTEGER NOT NULL,
    min_val     REAL, max_val REAL, avg_val REAL,
    count_val   INTEGER, sum_val REAL, sum_sq REAL,
    PRIMARY KEY (sensor_id, day_ts)
) WITHOUT ROWID;
"#;

// ─── Handle / global singleton ────────────────────────────────────────────────

pub struct Storage {
    conn: Mutex<Connection>,
    /// Current battery session id. All readings are tagged with this so
    /// we can group by unplug→plug intervals later.
    session_id: Mutex<i64>,
}

static STORAGE: OnceLock<Storage> = OnceLock::new();

pub fn init(path: &Path) -> SqlResult<()> {
    if STORAGE.get().is_some() {
        return Ok(());
    }
    let storage = Storage::open(path)?;
    STORAGE.set(storage).ok();
    Ok(())
}

pub fn global() -> Option<&'static Storage> {
    STORAGE.get()
}

/// Default database path: %LOCALAPPDATA%\BugJuice\bugjuice.db
pub fn default_db_path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("BugJuice").join("bugjuice.db")
}

// ─── Storage impl ─────────────────────────────────────────────────────────────

impl Storage {
    fn open(path: &Path) -> SqlResult<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;

        // PRAGMA config per scope doc. These must run once at open time.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA cache_size = -16000;
             PRAGMA mmap_size = 268435456;
             PRAGMA temp_store = MEMORY;
             PRAGMA busy_timeout = 5000;",
        )?;

        conn.execute_batch(SCHEMA)?;

        Ok(Storage {
            conn: Mutex::new(conn),
            session_id: Mutex::new(0),
        })
    }

    // ── Sensors ───────────────────────────────────────────────────────────────

    fn get_or_create_sensor(
        conn: &Connection,
        name: &str,
        unit: &str,
        category: &str,
        hw_source: Option<&str>,
    ) -> SqlResult<i64> {
        if let Some(id) = conn
            .query_row(
                "SELECT sensor_id FROM sensors WHERE name = ?",
                params![name],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
        {
            return Ok(id);
        }
        conn.execute(
            "INSERT INTO sensors (name, unit, category, hw_source) VALUES (?, ?, ?, ?)",
            params![name, unit, category, hw_source],
        )?;
        Ok(conn.last_insert_rowid())
    }

    // ── Readings ──────────────────────────────────────────────────────────────

    /// Single-row insert. Kept for callers that don't have a batch handy
    /// (events.rs, ad-hoc one-offs). The polling hot path uses
    /// `log_readings_batch` for fewer fsyncs.
    #[allow(dead_code)]
    pub fn log_reading(
        &self,
        name: &str,
        unit: &str,
        category: &str,
        hw_source: Option<&str>,
        value: f64,
    ) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        let sensor_id = Self::get_or_create_sensor(&conn, name, unit, category, hw_source)?;
        let session_id = *self.session_id.lock().unwrap();
        let ts = now_unix();
        conn.execute(
            "INSERT INTO readings (ts, sensor_id, value, session_id) VALUES (?, ?, ?, ?)",
            params![ts, sensor_id, value, session_id],
        )?;
        Ok(())
    }

    /// Insert many readings inside one transaction. Used by the polling
    /// loop so each tick is a single fsync instead of N. This is the
    /// performance-critical hot path; everything else can use `log_reading`.
    pub fn log_readings_batch(&self, readings: &[ReadingInput]) -> SqlResult<()> {
        if readings.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().unwrap();
        let session_id = *self.session_id.lock().unwrap();
        let ts = now_unix();
        let tx = conn.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO readings (ts, sensor_id, value, session_id) VALUES (?, ?, ?, ?)",
            )?;
            for r in readings {
                let sensor_id =
                    Self::get_or_create_sensor(&tx, &r.name, &r.unit, &r.category, r.hw_source)?;
                insert.execute(params![ts, sensor_id, r.value, session_id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // ── Battery sessions ──────────────────────────────────────────────────────

    /// Ensure we have a current battery session matching `on_ac`. If the
    /// existing session was on a different power source (or none exists),
    /// close the old one and start a new one.
    pub fn ensure_battery_session(&self, snap: &BatterySnapshot, on_ac: bool) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        let mut current = self.session_id.lock().unwrap();

        // Resume an open session from a previous run if present + matches.
        if *current == 0 {
            if let Some((id, existing_on_ac)) = conn
                .query_row(
                    "SELECT session_id, on_ac FROM battery_sessions
                     WHERE ended_at IS NULL ORDER BY started_at DESC LIMIT 1",
                    [],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? != 0)),
                )
                .optional()?
            {
                if existing_on_ac == on_ac {
                    *current = id;
                    return Ok(());
                }
                // Power state changed while we were off — close it.
                Self::close_session_locked(&conn, id, snap)?;
            }
        } else {
            // Already tracking — check if still matches.
            let existing_on_ac: i64 = conn.query_row(
                "SELECT on_ac FROM battery_sessions WHERE session_id = ?",
                params![*current],
                |r| r.get(0),
            )?;
            if (existing_on_ac != 0) == on_ac {
                return Ok(());
            }
            Self::close_session_locked(&conn, *current, snap)?;
        }

        // Start a fresh session.
        let ts = now_unix();
        let cap = normalize_capacity(snap.status.capacity);
        let pct = cap
            .map(|c| c as f64 / snap.info.full_charged_capacity.max(1) as f64 * 100.0);
        conn.execute(
            "INSERT INTO battery_sessions
             (started_at, start_percent, start_capacity, on_ac)
             VALUES (?, ?, ?, ?)",
            params![ts, pct, cap, on_ac as i64],
        )?;
        *current = conn.last_insert_rowid();
        Ok(())
    }

    fn close_session_locked(
        conn: &Connection,
        session_id: i64,
        snap: &BatterySnapshot,
    ) -> SqlResult<()> {
        let ts = now_unix();
        let cap = normalize_capacity(snap.status.capacity);
        let pct = cap
            .map(|c| c as f64 / snap.info.full_charged_capacity.max(1) as f64 * 100.0);

        // Compute average drain from readings in this session. Mean of the
        // "battery_rate" sensor over all rows belonging to this session,
        // where rate is stored as negative watts for discharge.
        let avg: Option<f64> = conn
            .query_row(
                "SELECT AVG(value) FROM readings r
                 JOIN sensors s ON r.sensor_id = s.sensor_id
                 WHERE r.session_id = ? AND s.name = 'battery_rate'",
                params![session_id],
                |r| r.get::<_, Option<f64>>(0),
            )
            .optional()?
            .flatten();

        conn.execute(
            "UPDATE battery_sessions
             SET ended_at = ?, end_percent = ?, end_capacity = ?, avg_drain_watts = ?
             WHERE session_id = ?",
            params![ts, pct, cap, avg, session_id],
        )?;
        Ok(())
    }

    // ── App power ─────────────────────────────────────────────────────────────

    /// Insert per-app power attributions for one polling tick. Single
    /// transaction so each tick is one fsync regardless of process count.
    pub fn log_app_power_batch(&self, rows: &[AppPowerRow]) -> SqlResult<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().unwrap();
        let ts = now_unix();
        let tx = conn.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO app_power
                 (ts, process_name, cpu_watts, gpu_watts, disk_watts, net_watts, total_watts)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )?;
            for r in rows {
                insert.execute(params![
                    ts,
                    r.process_name,
                    r.cpu_watts,
                    r.gpu_watts,
                    r.disk_watts,
                    r.net_watts,
                    r.total_watts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // ── Sleep sessions ────────────────────────────────────────────────────────

    /// Start a sleep session on PBT_APMSUSPEND. Returns the new row id.
    pub fn start_sleep_session(&self, pre_capacity: Option<u32>) -> SqlResult<i64> {
        let conn = self.conn.lock().unwrap();
        let ts = now_unix();
        conn.execute(
            "INSERT INTO sleep_sessions (sleep_at, pre_capacity) VALUES (?, ?)",
            params![ts, pre_capacity],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Finish a sleep session when the display actually comes back on.
    pub fn finish_sleep_session(
        &self,
        id: i64,
        post_capacity: Option<u32>,
        drain_mwh: Option<i64>,
        drain_rate_mw: Option<f64>,
        drain_percent: Option<f64>,
    ) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        let ts = now_unix();
        conn.execute(
            "UPDATE sleep_sessions
             SET wake_at = ?, post_capacity = ?, drain_mwh = ?,
                 drain_rate_mw = ?, drain_percent = ?
             WHERE id = ?",
            params![ts, post_capacity, drain_mwh, drain_rate_mw, drain_percent, id],
        )?;
        Ok(())
    }

    // ── Health snapshots ──────────────────────────────────────────────────────

    pub fn log_health_snapshot(&self, snap: &BatterySnapshot) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        let ts = now_unix();
        let wear = {
            let design = snap.info.designed_capacity.max(1) as f64;
            let full = snap.info.full_charged_capacity as f64;
            ((1.0 - full / design) * 100.0).max(0.0)
        };
        let voltage = if snap.status.voltage != BATTERY_UNKNOWN_CAPACITY {
            Some(snap.status.voltage as i64)
        } else {
            None
        };
        conn.execute(
            "INSERT INTO health_snapshots
             (ts, design_capacity, full_charge_capacity, cycle_count,
              wear_percent, voltage_mv, temperature_c)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                ts,
                snap.info.designed_capacity,
                snap.info.full_charged_capacity,
                snap.info.cycle_count,
                wear,
                voltage,
                snap.temperature_c,
            ],
        )?;
        Ok(())
    }

    // ── Tiered aggregation ─────────────────────────────────────────────────────

    /// Aggregate raw readings from the given hour into hourly_stats.
    /// hour_ts should be the start-of-hour Unix timestamp (aligned to 3600).
    pub fn aggregate_hour(&self, hour_ts: i64) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO hourly_stats (sensor_id, hour_ts, min_val, max_val, avg_val, count_val, sum_val, sum_sq)
             SELECT sensor_id, ?1, MIN(value), MAX(value), AVG(value), COUNT(*), SUM(value), SUM(value * value)
             FROM readings
             WHERE ts >= ?1 AND ts < ?2
             GROUP BY sensor_id",
            rusqlite::params![hour_ts, hour_ts + 3600],
        )
    }

    /// Aggregate hourly_stats from the given day into daily_stats.
    /// day_ts should be the start-of-day Unix timestamp (aligned to 86400).
    pub fn aggregate_day(&self, day_ts: i64) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO daily_stats (sensor_id, day_ts, min_val, max_val, avg_val, count_val, sum_val, sum_sq)
             SELECT sensor_id, ?1, MIN(min_val), MAX(max_val),
                    CASE WHEN SUM(count_val) > 0 THEN SUM(sum_val) / SUM(count_val) ELSE 0 END,
                    SUM(count_val), SUM(sum_val), SUM(sum_sq)
             FROM hourly_stats
             WHERE hour_ts >= ?1 AND hour_ts < ?2
             GROUP BY sensor_id",
            rusqlite::params![day_ts, day_ts + 86400],
        )
    }

    // ── Reads for the UI ──────────────────────────────────────────────────────

    /// Battery percent + rate readings over the last `seconds` seconds.
    /// Used by the dashboard sparkline.
    pub fn read_recent_history(&self, seconds: i64) -> SqlResult<Vec<HistoryPointRow>> {
        let conn = self.conn.lock().unwrap();
        let cutoff = now_unix() - seconds;
        let pct_rows: Vec<(i64, f64)> = {
            let mut stmt = conn.prepare(
                "SELECT r.ts, r.value
                 FROM readings r JOIN sensors s ON r.sensor_id = s.sensor_id
                 WHERE s.name = 'battery_percent' AND r.ts >= ?
                 ORDER BY r.ts ASC",
            )?;
            let mapped = stmt
                .query_map(params![cutoff], |r| Ok((r.get(0)?, r.get(1)?)))?
                .filter_map(|r| r.ok())
                .collect::<Vec<(i64, f64)>>();
            mapped
        };
        let rate_rows: std::collections::HashMap<i64, f64> = {
            let mut stmt = conn.prepare(
                "SELECT r.ts, r.value
                 FROM readings r JOIN sensors s ON r.sensor_id = s.sensor_id
                 WHERE s.name = 'battery_rate' AND r.ts >= ?
                 ORDER BY r.ts ASC",
            )?;
            let mapped = stmt
                .query_map(params![cutoff], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
                })?
                .filter_map(|r| r.ok())
                .collect::<std::collections::HashMap<i64, f64>>();
            mapped
        };
        Ok(pct_rows
            .into_iter()
            .map(|(ts, percent)| HistoryPointRow {
                ts,
                percent,
                rate_w: *rate_rows.get(&ts).unwrap_or(&0.0),
            })
            .collect())
    }

    /// Most recent app_power tick: returns rows from the latest `ts` only.
    pub fn read_recent_app_power(&self) -> SqlResult<Vec<AppPowerReadRow>> {
        let conn = self.conn.lock().unwrap();
        let max_ts: Option<i64> = conn
            .query_row("SELECT MAX(ts) FROM app_power", [], |r| r.get(0))
            .optional()?;
        let Some(max_ts) = max_ts else {
            return Ok(Vec::new());
        };
        let mut stmt = conn.prepare(
            "SELECT process_name, cpu_watts, gpu_watts, total_watts
             FROM app_power
             WHERE ts = ?
             ORDER BY total_watts DESC",
        )?;
        let v: Vec<AppPowerReadRow> = stmt
            .query_map(params![max_ts], |r| {
                Ok(AppPowerReadRow {
                    process_name: r.get(0)?,
                    cpu_watts: r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                    gpu_watts: r.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                    total_watts: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(v)
    }

    /// All battery sessions, newest first.
    pub fn read_battery_sessions(&self) -> SqlResult<Vec<BatterySessionRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT session_id, started_at, ended_at, start_percent, end_percent,
                    start_capacity, end_capacity, avg_drain_watts, on_ac
             FROM battery_sessions
             ORDER BY started_at DESC
             LIMIT 50",
        )?;
        let v: Vec<BatterySessionRow> = stmt
            .query_map([], |r| {
                Ok(BatterySessionRow {
                    id: r.get(0)?,
                    started_at: r.get(1)?,
                    ended_at: r.get(2)?,
                    start_percent: r.get(3)?,
                    end_percent: r.get(4)?,
                    start_capacity: r.get(5)?,
                    end_capacity: r.get(6)?,
                    avg_drain_watts: r.get(7)?,
                    on_ac: r.get::<_, i64>(8)? != 0,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(v)
    }

    /// All sleep sessions, newest first.
    pub fn read_sleep_sessions(&self) -> SqlResult<Vec<SleepSessionRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, sleep_at, wake_at, pre_capacity, post_capacity,
                    drain_mwh, drain_percent, drain_rate_mw, drips_percent
             FROM sleep_sessions
             ORDER BY sleep_at DESC
             LIMIT 50",
        )?;
        let v: Vec<SleepSessionRow> = stmt
            .query_map([], |r| {
                Ok(SleepSessionRow {
                    id: r.get(0)?,
                    sleep_at: r.get(1)?,
                    wake_at: r.get(2)?,
                    pre_capacity: r.get(3)?,
                    post_capacity: r.get(4)?,
                    drain_mwh: r.get(5)?,
                    drain_percent: r.get(6)?,
                    drain_rate_mw: r.get(7)?,
                    drips_percent: r.get(8)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(v)
    }

    /// Battery percent + rate readings within an absolute time range.
    /// Used by the unified timeline on the Sessions page.
    pub fn read_history_range(&self, start_ts: i64, end_ts: i64) -> SqlResult<Vec<HistoryPointRow>> {
        let conn = self.conn.lock().unwrap();
        let pct_rows: Vec<(i64, f64)> = {
            let mut stmt = conn.prepare(
                "SELECT r.ts, r.value
                 FROM readings r JOIN sensors s ON r.sensor_id = s.sensor_id
                 WHERE s.name = 'battery_percent' AND r.ts >= ? AND r.ts <= ?
                 ORDER BY r.ts ASC",
            )?;
            let mapped = stmt
                .query_map(params![start_ts, end_ts], |r| Ok((r.get(0)?, r.get(1)?)))?
                .filter_map(|r| r.ok())
                .collect::<Vec<(i64, f64)>>();
            mapped
        };
        let rate_rows: std::collections::HashMap<i64, f64> = {
            let mut stmt = conn.prepare(
                "SELECT r.ts, r.value
                 FROM readings r JOIN sensors s ON r.sensor_id = s.sensor_id
                 WHERE s.name = 'battery_rate' AND r.ts >= ? AND r.ts <= ?
                 ORDER BY r.ts ASC",
            )?;
            let mapped = stmt
                .query_map(params![start_ts, end_ts], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
                })?
                .filter_map(|r| r.ok())
                .collect::<std::collections::HashMap<i64, f64>>();
            mapped
        };
        Ok(pct_rows
            .into_iter()
            .map(|(ts, percent)| HistoryPointRow {
                ts,
                percent,
                rate_w: *rate_rows.get(&ts).unwrap_or(&0.0),
            })
            .collect())
    }

    /// Battery sessions overlapping a time range, newest first.
    pub fn read_battery_sessions_range(&self, start_ts: i64, end_ts: i64) -> SqlResult<Vec<BatterySessionRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT session_id, started_at, ended_at, start_percent, end_percent,
                    start_capacity, end_capacity, avg_drain_watts, on_ac
             FROM battery_sessions
             WHERE started_at <= ? AND (ended_at >= ? OR ended_at IS NULL)
             ORDER BY started_at DESC",
        )?;
        let v: Vec<BatterySessionRow> = stmt
            .query_map(params![end_ts, start_ts], |r| {
                Ok(BatterySessionRow {
                    id: r.get(0)?,
                    started_at: r.get(1)?,
                    ended_at: r.get(2)?,
                    start_percent: r.get(3)?,
                    end_percent: r.get(4)?,
                    start_capacity: r.get(5)?,
                    end_capacity: r.get(6)?,
                    avg_drain_watts: r.get(7)?,
                    on_ac: r.get::<_, i64>(8)? != 0,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(v)
    }

    /// Sleep sessions overlapping a time range, newest first.
    pub fn read_sleep_sessions_range(&self, start_ts: i64, end_ts: i64) -> SqlResult<Vec<SleepSessionRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, sleep_at, wake_at, pre_capacity, post_capacity,
                    drain_mwh, drain_percent, drain_rate_mw, drips_percent
             FROM sleep_sessions
             WHERE sleep_at <= ? AND (wake_at >= ? OR wake_at IS NULL)
             ORDER BY sleep_at DESC",
        )?;
        let v: Vec<SleepSessionRow> = stmt
            .query_map(params![end_ts, start_ts], |r| {
                Ok(SleepSessionRow {
                    id: r.get(0)?,
                    sleep_at: r.get(1)?,
                    wake_at: r.get(2)?,
                    pre_capacity: r.get(3)?,
                    post_capacity: r.get(4)?,
                    drain_mwh: r.get(5)?,
                    drain_percent: r.get(6)?,
                    drain_rate_mw: r.get(7)?,
                    drips_percent: r.get(8)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(v)
    }

    /// All health snapshots ordered oldest first (chart-friendly).
    pub fn read_health_history(&self) -> SqlResult<Vec<HealthSnapshotRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT ts, design_capacity, full_charge_capacity, cycle_count,
                    wear_percent, voltage_mv, temperature_c
             FROM health_snapshots
             ORDER BY ts ASC
             LIMIT 1000",
        )?;
        let v: Vec<HealthSnapshotRow> = stmt
            .query_map([], |r| {
                Ok(HealthSnapshotRow {
                    ts: r.get(0)?,
                    design_capacity: r.get(1)?,
                    full_charge_capacity: r.get(2)?,
                    cycle_count: r.get(3)?,
                    wear_percent: r.get(4)?,
                    voltage_mv: r.get(5)?,
                    temperature_c: r.get(6)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(v)
    }

    /// Per-session battery percent + rate history for the drill-down view.
    pub fn read_session_history(
        &self,
        session_id: i64,
    ) -> SqlResult<Vec<HistoryPointRow>> {
        let conn = self.conn.lock().unwrap();
        let pct_rows: Vec<(i64, f64)> = {
            let mut stmt = conn.prepare(
                "SELECT r.ts, r.value
                 FROM readings r JOIN sensors s ON r.sensor_id = s.sensor_id
                 WHERE r.session_id = ? AND s.name = 'battery_percent'
                 ORDER BY r.ts ASC",
            )?;
            let mapped = stmt
                .query_map(params![session_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .filter_map(|r| r.ok())
                .collect::<Vec<(i64, f64)>>();
            mapped
        };
        let rate_rows: std::collections::HashMap<i64, f64> = {
            let mut stmt = conn.prepare(
                "SELECT r.ts, r.value
                 FROM readings r JOIN sensors s ON r.sensor_id = s.sensor_id
                 WHERE r.session_id = ? AND s.name = 'battery_rate'
                 ORDER BY r.ts ASC",
            )?;
            let mapped = stmt
                .query_map(params![session_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .filter_map(|r| r.ok())
                .collect::<std::collections::HashMap<i64, f64>>();
            mapped
        };
        Ok(pct_rows
            .into_iter()
            .map(|(ts, percent)| HistoryPointRow {
                ts,
                percent,
                rate_w: *rate_rows.get(&ts).unwrap_or(&0.0),
            })
            .collect())
    }

    /// Per-power-channel readings over the last `seconds`. Returns
    /// (sensor_name, [(ts, watts), …]) for everything in the 'power'
    /// category. The Components page groups these into stacked series.
    pub fn read_power_history(
        &self,
        seconds: i64,
    ) -> SqlResult<Vec<(String, Vec<(i64, f64)>)>> {
        let conn = self.conn.lock().unwrap();
        let cutoff = now_unix() - seconds;
        let mut stmt = conn.prepare(
            "SELECT s.name, r.ts, r.value
             FROM readings r JOIN sensors s ON r.sensor_id = s.sensor_id
             WHERE s.category = 'power' AND r.ts >= ?
             ORDER BY r.ts ASC",
        )?;
        let mut grouped: std::collections::HashMap<String, Vec<(i64, f64)>> =
            std::collections::HashMap::new();
        for row in stmt.query_map(params![cutoff], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, f64>(2)?))
        })? {
            if let Ok((name, ts, val)) = row {
                grouped.entry(name).or_default().push((ts, val));
            }
        }
        Ok(grouped.into_iter().collect())
    }

    // ── Stats / queries ───────────────────────────────────────────────────────

    pub fn row_counts(&self) -> SqlResult<RowCounts> {
        let conn = self.conn.lock().unwrap();
        let readings: i64 = conn.query_row("SELECT COUNT(*) FROM readings", [], |r| r.get(0))?;
        let sessions: i64 =
            conn.query_row("SELECT COUNT(*) FROM battery_sessions", [], |r| r.get(0))?;
        let sleeps: i64 =
            conn.query_row("SELECT COUNT(*) FROM sleep_sessions", [], |r| r.get(0))?;
        let health: i64 =
            conn.query_row("SELECT COUNT(*) FROM health_snapshots", [], |r| r.get(0))?;
        let sensors: i64 = conn.query_row("SELECT COUNT(*) FROM sensors", [], |r| r.get(0))?;
        let app_power: i64 = conn.query_row("SELECT COUNT(*) FROM app_power", [], |r| r.get(0))?;
        Ok(RowCounts {
            readings,
            battery_sessions: sessions,
            sleep_sessions: sleeps,
            health_snapshots: health,
            sensors,
            app_power,
        })
    }

    /// Aggregated per-process power stats for a time range. Returns the
    /// top 20 processes by average wattage.
    pub fn read_app_power_summary(
        &self,
        start_ts: i64,
        end_ts: i64,
    ) -> rusqlite::Result<Vec<AppPowerSummaryRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT process_name,
                    AVG(COALESCE(total_watts, 0)) as avg_watts,
                    MAX(COALESCE(total_watts, 0)) as max_watts,
                    COUNT(*) as sample_count
             FROM app_power
             WHERE ts >= ?1 AND ts < ?2
             GROUP BY process_name
             ORDER BY avg_watts DESC
             LIMIT 20",
        )?;
        let rows = stmt.query_map(rusqlite::params![start_ts, end_ts], |row| {
            Ok(AppPowerSummaryRow {
                process_name: row.get(0)?,
                avg_watts: row.get(1)?,
                max_watts: row.get(2)?,
                sample_count: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// Per-power-channel readings within an absolute time range. Returns
    /// (sensor_name, [(ts, watts), …]) for everything in the 'power'
    /// category. Range-based variant of `read_power_history`.
    pub fn read_power_history_range(
        &self,
        start_ts: i64,
        end_ts: i64,
    ) -> SqlResult<Vec<(String, Vec<(i64, f64)>)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.name, r.ts, r.value
             FROM readings r JOIN sensors s ON r.sensor_id = s.sensor_id
             WHERE s.category = 'power' AND r.ts >= ? AND r.ts < ?
             ORDER BY r.ts ASC",
        )?;
        let mut grouped: std::collections::HashMap<String, Vec<(i64, f64)>> =
            std::collections::HashMap::new();
        for row in stmt.query_map(params![start_ts, end_ts], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, f64>(2)?,
            ))
        })? {
            if let Ok((name, ts, val)) = row {
                grouped.entry(name).or_default().push((ts, val));
            }
        }
        Ok(grouped.into_iter().collect())
    }
}

pub struct RowCounts {
    pub readings: i64,
    pub battery_sessions: i64,
    pub sleep_sessions: i64,
    pub health_snapshots: i64,
    pub sensors: i64,
    pub app_power: i64,
}

/// One sensor reading queued for batch insert.
pub struct ReadingInput<'a> {
    pub name: String,
    pub unit: &'a str,
    pub category: &'a str,
    pub hw_source: Option<&'a str>,
    pub value: f64,
}

/// One row of per-process power attribution for the app_power table.
/// Per-component fields are optional so we can fill them in as the
/// estimation engine grows (CPU first, GPU/disk/net later).
pub struct AppPowerRow {
    pub process_name: String,
    pub cpu_watts: Option<f64>,
    pub gpu_watts: Option<f64>,
    pub disk_watts: Option<f64>,
    pub net_watts: Option<f64>,
    pub total_watts: Option<f64>,
}

// ─── Read row types (returned to the Tauri command layer) ────────────────────

#[derive(Debug, Clone)]
pub struct HistoryPointRow {
    pub ts: i64,
    pub percent: f64,
    pub rate_w: f64,
}

#[derive(Debug, Clone)]
pub struct AppPowerReadRow {
    pub process_name: String,
    pub cpu_watts: f64,
    pub gpu_watts: f64,
    pub total_watts: f64,
}

#[derive(Debug, Clone)]
pub struct BatterySessionRow {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub start_percent: Option<f64>,
    pub end_percent: Option<f64>,
    pub start_capacity: Option<i64>,
    pub end_capacity: Option<i64>,
    pub avg_drain_watts: Option<f64>,
    pub on_ac: bool,
}

#[derive(Debug, Clone)]
pub struct SleepSessionRow {
    pub id: i64,
    pub sleep_at: i64,
    pub wake_at: Option<i64>,
    pub pre_capacity: Option<i64>,
    pub post_capacity: Option<i64>,
    pub drain_mwh: Option<i64>,
    pub drain_percent: Option<f64>,
    pub drain_rate_mw: Option<f64>,
    pub drips_percent: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct HealthSnapshotRow {
    pub ts: i64,
    pub design_capacity: i64,
    pub full_charge_capacity: i64,
    pub cycle_count: Option<i64>,
    pub wear_percent: Option<f64>,
    pub voltage_mv: Option<i64>,
    pub temperature_c: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct AppPowerSummaryRow {
    pub process_name: String,
    pub avg_watts: f64,
    pub max_watts: f64,
    pub sample_count: i64,
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn normalize_capacity(cap: u32) -> Option<u32> {
    if cap == BATTERY_UNKNOWN_CAPACITY {
        None
    } else {
        Some(cap)
    }
}
