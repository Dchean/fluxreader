use super::*;
use rusqlite::params;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct FolderRow {
    pub id: i64,
    pub name: String,
    pub layout: String,
    pub auto_summary: bool,
    pub auto_translate: bool,
    pub collapsed: bool,
    pub position: i64,
}

pub fn list_folders(conn: &Connection) -> AppResult<Vec<FolderRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, layout, auto_summary, auto_translate, collapsed, position
         FROM folders ORDER BY position, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(FolderRow {
            id: r.get(0)?,
            name: r.get(1)?,
            layout: r.get(2)?,
            auto_summary: r.get::<_, i64>(3)? != 0,
            auto_translate: r.get::<_, i64>(4)? != 0,
            collapsed: r.get::<_, i64>(5)? != 0,
            position: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn create_folder(conn: &Connection, name: &str, layout: &str) -> AppResult<i64> {
    let next_pos: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM folders",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO folders (name, position, layout, collapsed) VALUES (?1, ?2, ?3, 1)",
        params![name, next_pos, layout],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn rename_folder(conn: &Connection, id: i64, name: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE folders SET name = ?1 WHERE id = ?2",
        params![name, id],
    )?;
    Ok(())
}

pub fn delete_folder(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM folders WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn update_folder_layout(conn: &Connection, id: i64, layout: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE folders SET layout = ?1 WHERE id = ?2",
        params![layout, id],
    )?;
    Ok(())
}

pub fn set_folder_collapsed(conn: &Connection, id: i64, collapsed: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE folders SET collapsed = ?1 WHERE id = ?2",
        params![collapsed as i64, id],
    )?;
    Ok(())
}

pub fn set_folder_ai_flags(
    conn: &Connection,
    id: i64,
    summary: bool,
    translate: bool,
) -> AppResult<()> {
    conn.execute(
        "UPDATE folders SET auto_summary = ?1, auto_translate = ?2 WHERE id = ?3",
        params![summary as i64, translate as i64, id],
    )?;
    Ok(())
}

/* ============================================================
Feeds
============================================================ */

/// 分类名（id → name）：订阅编辑推送远端时用于 `a=` 目标分类参数。
pub fn folder_name(conn: &Connection, id: i64) -> AppResult<Option<String>> {
    Ok(conn
        .query_row("SELECT name FROM folders WHERE id = ?1", [id], |r| r.get(0))
        .ok())
}
