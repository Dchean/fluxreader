// 独立复核：真实 list_articles_sql 形态（articles.rs:202/227）
fn article_where() -> (Vec<&'static str>, Vec<i64>) { (vec![], vec![]) }
pub fn list_articles_sql(newest_first: bool) -> String {
    let mut sql = String::from("SELECT");
    let (where_clauses, mut params) = article_where();
    let predicate: &'static str = "(a.published_at < ?)";
    if newest_first {
        where_clauses.push(predicate);
        params.push(1);
    }
    if !where_clauses.is_empty() { sql.push_str(" WHERE "); }
    sql
}
