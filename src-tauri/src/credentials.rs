//! 凭据加密存储（SEC-2）：Windows DPAPI 加密，读 DB 不再见明文。
//!
//! 覆盖四类敏感值（settings 键）：
//! - `miniflux_token`（历史遗留：旧 Miniflux API token，已停用但保留加密）
//! - `greader_password`（Google Reader 集成密码）
//! - `ai_config`（AI 服务配置，含 api key）
//! - `config_sync_credentials`（Gist PAT / WebDAV 密码）
//!
//! 方案（OPT-014 / F22 **失败关闭**）：Windows 上用 DPAPI（`CryptProtectData`，
//! 绑定当前用户，无需自管密钥），密文以 `dpapi:` 前缀 base64 存 SQLite。
//! 加密或解密失败都返回 Err——绝不在失败时把明文落库（用户以为凭据受静态加密
//! 保护、实际明文落盘是更糟的失败策略），也绝不把 `dpapi:` 原串当「已解密值」
//! 交给调用方（那会把密文当密码发给服务端）。历史明文值（无前缀）读取时保持
//! 兼容（迁移的前置条件），下次写入自动升级为密文。
//! 非 Windows 平台无 DPAPI：保持**显式开发降级口径**（明文 + 日志警示，
//! 生产目标平台为 Windows），不影响 Windows 生产失败关闭。
//!
//! Note: 失败关闭边界、为什么不悄悄降级——见
//! .agents/notes/implemented/architecture/2026-10-08-凭据失败关闭与受控更新检查.md

use crate::error::{AppError, AppResult};

/// 密文前缀标记：以 `dpapi:` 开头说明是 DPAPI 密文，否则视为明文（历史遗留/非 Windows）。
const DPAPI_PREFIX: &str = "dpapi:";

/// 需要加密存储的 settings 键（SEC-2）。
pub const SENSITIVE_KEYS: &[&str] = &[
    "miniflux_token",
    "greader_password",
    "ai_config",
    "config_sync_credentials",
];

/// 判断某 settings 键是否需要加解密。
pub fn is_sensitive_key(key: &str) -> bool {
    SENSITIVE_KEYS.contains(&key)
}

/// 启动迁移：把历史明文敏感值升级为 DPAPI 密文（SEC-2）。
///
/// 旧版本把 miniflux_token/ai_config/config_sync_credentials 明文存 SQLite。
/// 成功路径见 [`migrate_legacy_plaintext_with`]；失败必须整体 Err——调用方
/// （`db::open`）以 `?` 传播：宁可启动时可见地失败，也不悄悄把明文留在库里。
///
/// 非 Windows 平台无 DPAPI，encrypt_secret 只会回落明文（无法产生 `dpapi:`
/// 前缀），迁移永远无法"完成"——每次都重写相同的明文，幂等语义被破坏。
/// 故非 Windows 直接跳过迁移（返回 0），保持幂等（与 encrypt_secret 的回落
/// 行为一致：生产目标平台是 Windows，非 Windows 仅开发态，无需加密）。
pub fn migrate_legacy_plaintext(conn: &rusqlite::Connection) -> AppResult<usize> {
    #[cfg(not(windows))]
    {
        let _ = conn;
        Ok(0)
    }
    #[cfg(windows)]
    {
        migrate_legacy_plaintext_with(conn, encrypt_secret)
    }
}

/// 迁移核心（私有注入点）：扫描 SENSITIVE_KEYS 全部键，凡无 `dpapi:` 前缀
/// 且非空的，用 `encrypt` 重写并计数。幂等：已密文值跳过。
///
/// 失败关闭（F22）：任一键加密失败立即整体 Err——**不得**写入假密文，也不得把
/// 该键计入 upgraded（「升级成功」必须为真）；已升级的键保持幂等，下次启动
/// 对失败键重试。注入参数仅本模块单测使用——无 pub 入口，webview/其他 Rust
/// 模块均不可操控加密后端。
fn migrate_legacy_plaintext_with(
    conn: &rusqlite::Connection,
    encrypt: fn(&str) -> AppResult<String>,
) -> AppResult<usize> {
    use rusqlite::OptionalExtension;
    let mut upgraded = 0usize;
    for key in SENSITIVE_KEYS {
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                rusqlite::params![key],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(raw) = raw {
            if raw.is_empty() || raw.starts_with(DPAPI_PREFIX) {
                continue;
            }
            let enc = encrypt(&raw)?;
            conn.execute(
                "UPDATE settings SET value = ?1 WHERE key = ?2",
                rusqlite::params![enc, key],
            )?;
            upgraded += 1;
        }
    }
    Ok(upgraded)
}

/// 加密一段敏感值。
/// - 空串与已密文值幂等返回；
/// - Windows：DPAPI 加密失败 → Err（失败关闭，**不回退明文**）；
/// - 非 Windows：无 DPAPI，显式开发降级为明文（+ 日志警示）。
pub fn encrypt_secret(plain: &str) -> AppResult<String> {
    if plain.is_empty() || plain.starts_with(DPAPI_PREFIX) {
        return Ok(plain.to_string());
    }
    #[cfg(windows)]
    {
        encrypt_secret_with(plain, dpapi_encrypt)
    }
    #[cfg(not(windows))]
    {
        log::warn!("非 Windows 平台无 DPAPI，凭据以明文存储（仅开发态，生产请用 Windows）");
        Ok(plain.to_string())
    }
}

/// DPAPI 加密核心（私有注入点）：成功返回 `dpapi:<base64>`。
/// `encrypt` 失败整体 Err——**绝不允许**回退明文（F22）；错误信息只含后端错误
/// 描述，不含明文。注入参数仅本模块单测使用——无 pub 入口，
/// webview/其他 Rust 模块均不可操控加密后端。
#[cfg(windows)]
fn encrypt_secret_with(
    plain: &str,
    encrypt: fn(&[u8]) -> Result<Vec<u8>, String>,
) -> AppResult<String> {
    if plain.is_empty() || plain.starts_with(DPAPI_PREFIX) {
        return Ok(plain.to_string());
    }
    let cipher = encrypt(plain.as_bytes())
        .map_err(|e| AppError::new("credentialEncrypt", format!("凭据加密失败，未保存：{e}")))?;
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&cipher);
    Ok(format!("{DPAPI_PREFIX}{b64}"))
}

/// 解密一段存库值。
/// - `dpapi:` 前缀走 DPAPI 解密；base64 错误/DPAPI 失败 → Err（可见错误，
///   不把 `dpapi:` 原串当密码发送）；
/// - 无前缀（历史明文/非 Windows）原样返回——保留明文 legacy 读取以便迁移，
///   不能误改有效旧凭据。
pub fn decrypt_secret(stored: &str) -> AppResult<String> {
    if stored.is_empty() {
        return Ok(String::new());
    }
    if let Some(b64) = stored.strip_prefix(DPAPI_PREFIX) {
        #[cfg(windows)]
        {
            use base64::Engine;
            let cipher = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| {
                    AppError::new(
                        "credentialCorrupt",
                        format!("凭据密文格式损坏（base64 解析失败）：{e}"),
                    )
                })?;
            let plain = dpapi_decrypt(&cipher).map_err(|e| {
                AppError::new(
                    "credentialDecrypt",
                    format!("凭据 DPAPI 解密失败（可能已损坏或跨用户拷贝）：{e}"),
                )
            })?;
            return Ok(String::from_utf8_lossy(&plain).into_owned());
        }
        #[cfg(not(windows))]
        {
            // 非 Windows 无 DPAPI：不能把密文原文当"已解密值"交给调用方
            let _ = b64;
            return Err(AppError::new(
                "credentialDecrypt",
                "非 Windows 平台无法解密 DPAPI 密文（仅开发态口径）",
            ));
        }
    }
    // 无前缀：历史明文或非 Windows 明文
    Ok(stored.to_string())
}

#[cfg(windows)]
fn dpapi_encrypt(plain: &[u8]) -> Result<Vec<u8>, String> {
    use windows::core::w;
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: plain.len() as u32,
        pbData: plain.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // szdatadescr 描述串（应用名，绑定到密文元数据，非必须但便于识别）
    let desc = w!("FluxReader");
    unsafe {
        let res = CryptProtectData(
            &in_blob,
            desc,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        );
        if let Err(e) = res {
            return Err(format!("CryptProtectData 失败: {e}"));
        }
        let bytes = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        // 释放 DPAPI 分配的内存（HLOCAL 句柄）
        let _ = windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(
            out_blob.pbData as *mut core::ffi::c_void,
        )));
        Ok(bytes)
    }
}

#[cfg(windows)]
fn dpapi_decrypt(cipher: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: cipher.len() as u32,
        pbData: cipher.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let mut desc: windows::core::PWSTR = windows::core::PWSTR(std::ptr::null_mut());
    unsafe {
        let res = CryptUnprotectData(
            &in_blob,
            Some(&mut desc),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        );
        if let Err(e) = res {
            return Err(format!("CryptUnprotectData 失败: {e}"));
        }
        let bytes = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        let _ = windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(
            out_blob.pbData as *mut core::ffi::c_void,
        )));
        // 释放描述串（CryptUnprotectData 分配）
        if !desc.0.is_null() {
            let _ = windows::Win32::Foundation::LocalFree(Some(
                windows::Win32::Foundation::HLOCAL(desc.0 as *mut core::ffi::c_void),
            ));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let plain = "sensitive-token-123";
        let enc = encrypt_secret(plain).expect("加密应成功");
        // 平台语义：Windows 上 DPAPI 加密（enc 不再是明文、带 dpapi: 前缀）；
        // 非 Windows 回落明文（enc == plain）。两个分支都要验证往返解密正确。
        #[cfg(windows)]
        {
            assert_ne!(enc, plain, "Windows 上加密后不应是明文");
            assert!(enc.starts_with(DPAPI_PREFIX));
        }
        #[cfg(not(windows))]
        {
            assert_eq!(enc, plain, "非 Windows 无 DPAPI，回落明文");
        }
        let dec = decrypt_secret(&enc).expect("解密应成功");
        assert_eq!(dec, plain, "解密往返应还原明文");
    }

    #[test]
    fn empty_and_legacy_plaintext_passthrough() {
        assert_eq!(encrypt_secret("").unwrap(), "");
        // 历史明文（无前缀）读时原样返回，保证兼容
        assert_eq!(decrypt_secret("legacy-plain").unwrap(), "legacy-plain");
    }

    #[test]
    fn idempotent_on_already_encrypted() {
        let enc = encrypt_secret("abc").unwrap();
        let enc2 = encrypt_secret(&enc).unwrap();
        assert_eq!(enc, enc2, "已密文值再加密应幂等");
    }

    /// OPT-014 / F22：DPAPI 加密失败必须失败关闭——Err 且不回退明文，
    /// 错误信息不含明文；成功注入产出 `dpapi:` 前缀（加密格式契约）。
    #[cfg(windows)]
    #[test]
    fn encrypt_failure_is_error_not_plaintext_fallback() {
        let err = encrypt_secret_with("would-be-secret", |_| Err("注入的 DPAPI 失败".to_string()))
            .expect_err("DPAPI 失败必须 Err，而不是回退明文");
        assert_eq!(err.code, "credentialEncrypt");
        assert!(!err.message.contains("would-be-secret"), "错误不得含明文");

        // 对照：成功后端必须产出 `dpapi:<base64>` 形态（前端/存储层依赖的前缀契约）
        let ok = encrypt_secret_with("x", |bytes| Ok(bytes.to_vec())).unwrap();
        assert!(
            ok.starts_with(DPAPI_PREFIX),
            "成功加密必须带 dpapi: 前缀：{ok}"
        );
    }

    /// 损坏密文读取：base64 错误与 DPAPI 失败都是可见 Err，不透传原串。
    #[cfg(windows)]
    #[test]
    fn corrupted_ciphertext_is_error() {
        assert_eq!(
            decrypt_secret("dpapi:@@bad@@").unwrap_err().code,
            "credentialCorrupt"
        );
        assert_eq!(
            decrypt_secret("dpapi:bm90LWEtY2lwaGVy").unwrap_err().code,
            "credentialDecrypt"
        );
    }

    /// 非 Windows 开发降级口径：明文写入保持可用；dpapi: 密文无法解密 → Err
    /// （绝不能把密文原文当已解密值返回）。
    #[cfg(not(windows))]
    #[test]
    fn no_dpapi_platform_is_explicit_dev_fallback_and_cannot_decrypt() {
        assert_eq!(encrypt_secret("dev-plain").unwrap(), "dev-plain");
        let err = decrypt_secret("dpapi:eA==").unwrap_err();
        assert_eq!(err.code, "credentialDecrypt");
    }

    /// 启动迁移：历史明文敏感键被升级为密文，且幂等。
    /// 非 Windows 平台无 DPAPI，迁移直接跳过（返回 0），明文保持不变。
    #[test]
    fn migrate_legacy_plaintext_upgrades_and_idempotent() {
        use crate::db;
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        db::MIGRATIONS.to_latest(&mut conn).unwrap();

        // 用裸 SQL 模拟历史明文（绕过 set_setting 的加密）
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('miniflux_token', 'plain-token')",
            [],
        )
        .unwrap();
        // 迁移一次
        let n1 = migrate_legacy_plaintext(&conn).unwrap();
        #[cfg(windows)]
        assert_eq!(n1, 1, "首次迁移升级 1 个键");
        #[cfg(not(windows))]
        assert_eq!(n1, 0, "非 Windows 无 DPAPI，迁移跳过");
        let stored: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key='miniflux_token'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // 平台分支：Windows 上应为密文（dpapi: 前缀），非 Windows 保持明文。
        // 两个分支都要引用 stored，避免 Linux（cfg(windows)=false）下 stored 未使用
        // 触发 unused_variables（clippy -D warnings 下 CI 失败）。
        #[cfg(windows)]
        assert!(stored.starts_with(DPAPI_PREFIX), "迁移后应为密文: {stored}");
        #[cfg(not(windows))]
        assert_eq!(stored, "plain-token", "非 Windows 平台应保持明文");
        // 幂等：再迁移不重复升级
        let n2 = migrate_legacy_plaintext(&conn).unwrap();
        assert_eq!(n2, 0, "二次迁移不重复升级");
    }

    /// OPT-014 / F22 迁移负例（注入）：加密失败 → 整体 Err、不写假密文、不计数；
    /// 随后成功加密器可正常升级并计数，已密文后幂等（失败注入器不再被调用）。
    #[test]
    fn migrate_failure_injection_keeps_plaintext_and_does_not_count() {
        use crate::db;
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        db::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('miniflux_token', 'plain-token')",
            [],
        )
        .unwrap();

        let err = migrate_legacy_plaintext_with(&conn, |_| {
            Err(AppError::new("credentialEncrypt", "注入的加密失败"))
        })
        .expect_err("迁移加密失败必须整体 Err");
        assert_eq!(err.code, "credentialEncrypt");
        let stored: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key='miniflux_token'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, "plain-token", "失败后必须保持原明文（不写假密文）");

        let n = migrate_legacy_plaintext_with(&conn, |plain| {
            Ok(format!("{DPAPI_PREFIX}test-masked[{}]", plain.len()))
        })
        .unwrap();
        assert_eq!(n, 1, "恢复路径应升级并计数");
        let stored2: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key='miniflux_token'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(stored2.starts_with(DPAPI_PREFIX));
        assert_eq!(
            migrate_legacy_plaintext_with(&conn, |_| Err(AppError::new(
                "credentialEncrypt",
                "不应再被调用"
            )))
            .unwrap(),
            0,
            "已密文后幂等，失败注入器不应被调用"
        );
    }
}
