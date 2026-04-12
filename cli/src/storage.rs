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
        Ok(RowCounts {
            readings,
            battery_sessions: sessions,
            sleep_sessions: sleeps,
            health_snapshots: health,
            sensors,
        })
    }
}

pub struct RowCounts {
    pub readings: i64,
    pub battery_sessions: i64,
    pub sleep_sessions: i64,
    pub health_snapshots: i64,
    pub sensors: i64,
}

/// One sensor reading queued for batch insert.
pub struct ReadingInput<'a> {
    pub name: String,
    pub unit: &'a str,
    pub category: &'a str,
    pub hw_source: Option<&'a str>,
    pub value: f64,
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
