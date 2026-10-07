// 复刻 src/db/articles.rs:202-229 真实形态（Value 为 rusqlite::types::Value 桩）
#[derive(Debug, Clone)]
pub enum Value { Text(String), Integer(i64) }
impl From<String> for Value { fn from(s: String) -> Self { Value::Text(s) } }
impl From<i64> for Value { fn from(i: i64) -> Self { Value::Integer(i) } }

const COLS: &str = "a.id, a.title";
const KEYSET_PREDICATE_DESC: &str = "(a.published_at < ? OR (a.published_at = ? AND a.id < ?))";
const KEYSET_PREDICATE_ASC: &str = "(a.published_at > ? OR (a.published_at = ? AND a.id > ?))";

pub struct Q { newest_first: bool, last_published: Option<String>, last_id: Option<i64> }

fn article_where() -> (Vec<&'static str>, Vec<Value>) { (vec![], vec![]) }

pub fn list_articles_sql(q: &Q) -> (String, Vec<Value>) {
    let mut sql = format!("SELECT {COLS} FROM articles a");
    let (mut where_clauses, mut params) = article_where();
    if let (Some(last_published), Some(last_id)) = (&q.last_published, q.last_id) {
        let (predicate, dir_params): (&'static str, Vec<Value>) = if q.newest_first {
            (
                KEYSET_PREDICATE_DESC,
                vec![
                    last_published.clone().into(),
                    last_published.clone().into(),
                    last_id.into(),
                ],
            )
        } else {
            (
                KEYSET_PREDICATE_ASC,
                vec![
                    last_published.clone().into(),
                    last_published.clone().into(),
                    last_id.into(),
                ],
            )
        };
        where_clauses.push(predicate);
        params.extend(dir_params);
    }
    if !where_clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_clauses.join(" AND "));
    }
    (sql, params)
}
