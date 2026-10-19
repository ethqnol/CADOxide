//! Database Schema & Initialization Engine for CADoxide Server

use rusqlite::Connection;

pub fn init_db(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS organizations (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            slug TEXT UNIQUE NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            email TEXT UNIQUE NOT NULL,
            full_name TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS projects (
            id TEXT PRIMARY KEY,
            org_id TEXT NOT NULL,
            name TEXT NOT NULL,
            slug TEXT NOT NULL,
            description TEXT,
            visibility TEXT NOT NULL DEFAULT 'private',
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS design_revisions (
            id TEXT PRIMARY KEY,
            project_slug TEXT NOT NULL,
            git_commit_hash TEXT NOT NULL,
            git_branch TEXT NOT NULL,
            author_name TEXT NOT NULL,
            commit_message TEXT NOT NULL,
            render_status TEXT NOT NULL DEFAULT 'completed',
            glb_3d_asset_url TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS parts_library (
            id TEXT PRIMARY KEY,
            mpn TEXT NOT NULL,
            manufacturer TEXT NOT NULL,
            description TEXT NOT NULL,
            value TEXT NOT NULL,
            footprint_name TEXT NOT NULL,
            symbol_name TEXT NOT NULL,
            lifecycle_status TEXT NOT NULL DEFAULT 'active',
            unit_price_usd REAL NOT NULL,
            digikey_stock INTEGER NOT NULL,
            mouser_stock INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_parts_mpn ON parts_library(mpn);

        CREATE TABLE IF NOT EXISTS review_threads (
            id TEXT PRIMARY KEY,
            revision_id TEXT NOT NULL,
            file_type TEXT NOT NULL,
            sheet_or_layer TEXT NOT NULL,
            coord_x REAL NOT NULL,
            coord_y REAL NOT NULL,
            status TEXT NOT NULL DEFAULT 'open',
            created_by TEXT NOT NULL,
            created_at TEXT NOT NULL,
            resolved_at TEXT
        );

        CREATE TABLE IF NOT EXISTS review_messages (
            id TEXT PRIMARY KEY,
            thread_id TEXT NOT NULL REFERENCES review_threads(id) ON DELETE CASCADE,
            user_id TEXT NOT NULL,
            user_name TEXT NOT NULL,
            body TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        "#,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_db_creates_all_tables() {
        let conn = Connection::open_in_memory().expect("In-memory SQLite connection");
        init_db(&conn).expect("Database initialization succeeds");

        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(tables.contains(&"organizations".to_string()));
        assert!(tables.contains(&"users".to_string()));
        assert!(tables.contains(&"projects".to_string()));
        assert!(tables.contains(&"design_revisions".to_string()));
        assert!(tables.contains(&"parts_library".to_string()));
        assert!(tables.contains(&"review_threads".to_string()));
        assert!(tables.contains(&"review_messages".to_string()));
    }
}

