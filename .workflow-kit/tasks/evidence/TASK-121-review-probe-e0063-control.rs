// 独立复核：tests/ingestion_e2e.rs:95 修前形态（12 字段字面量缺 2）
pub struct Q { pub a: i64, pub b: bool, pub last_published: Option<String>, pub last_id: Option<i64> }
pub fn build() -> Q {
    Q { a: 1, b: false }
}
