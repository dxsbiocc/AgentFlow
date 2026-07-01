use rusqlite::{params, Connection};

use super::project_store::{now_unix_seconds, StorageError};
use super::schema::{
    MODULES_TABLE_SQL, RUN_ATTEMPT_JOB_HANDLE_SQL, SCHEMA_MIGRATIONS_SQL, V0_SCHEMA_SQL,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationRecord {
    pub version: i64,
    pub name: String,
    pub checksum: String,
    pub applied_at: i64,
}

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "create_v0_schema",
        sql: V0_SCHEMA_SQL,
    },
    Migration {
        version: 2,
        name: "create_modules_table",
        sql: MODULES_TABLE_SQL,
    },
    Migration {
        version: 3,
        name: "run_attempt_job_handle",
        sql: RUN_ATTEMPT_JOB_HANDLE_SQL,
    },
];

pub fn apply_migrations(conn: &Connection) -> Result<(), StorageError> {
    conn.execute_batch(SCHEMA_MIGRATIONS_SQL)?;
    validate_applied_migrations(conn)?;

    for migration in MIGRATIONS {
        if is_applied(conn, migration.version)? {
            continue;
        }

        conn.execute_batch(migration.sql)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                migration.version,
                migration.name,
                checksum(migration.sql),
                now_unix_seconds()
            ],
        )?;
    }

    Ok(())
}

fn validate_applied_migrations(conn: &Connection) -> Result<(), StorageError> {
    let records = applied_migrations(conn)?;
    let latest_supported = MIGRATIONS
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or(0);

    for record in records {
        if record.version > latest_supported {
            return Err(StorageError::IncompatibleSchema(format!(
                "database has migration version {}, but this engine supports up to {}",
                record.version, latest_supported
            )));
        }

        let Some(expected) = MIGRATIONS
            .iter()
            .find(|migration| migration.version == record.version)
        else {
            return Err(StorageError::IncompatibleSchema(format!(
                "database contains unknown migration version {}",
                record.version
            )));
        };

        let expected_checksum = checksum(expected.sql);
        if record.checksum != expected_checksum {
            return Err(StorageError::IncompatibleSchema(format!(
                "migration {} checksum mismatch",
                record.version
            )));
        }
    }

    Ok(())
}

pub fn applied_migrations(conn: &Connection) -> Result<Vec<MigrationRecord>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT version, name, checksum, applied_at
         FROM schema_migrations
         ORDER BY version ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(MigrationRecord {
            version: row.get(0)?,
            name: row.get(1)?,
            checksum: row.get(2)?,
            applied_at: row.get(3)?,
        })
    })?;

    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}

pub fn checksum(input: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

fn is_applied(conn: &Connection, version: i64) -> Result<bool, StorageError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
        [version],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_is_stable() {
        assert_eq!(checksum("abc"), checksum("abc"));
        assert_ne!(checksum("abc"), checksum("abcd"));
    }

    #[test]
    fn future_migration_version_is_rejected() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at)
             VALUES (999, 'future', 'future-checksum', 0)",
            [],
        )
        .unwrap();

        let err = apply_migrations(&conn).unwrap_err();
        assert!(err.to_string().contains("supports up to 3"));
    }

    #[test]
    fn checksum_mismatch_is_rejected() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_MIGRATIONS_SQL).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at)
             VALUES (1, 'create_v0_schema', 'wrong-checksum', 0)",
            [],
        )
        .unwrap();

        let err = apply_migrations(&conn).unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"));
    }
}
