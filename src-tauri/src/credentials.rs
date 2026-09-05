//! 凭据加密存储（SEC-2）：Windows DPAPI 加密，读 DB 不再见明文。
//!
//! 覆盖三类敏感值（settings 键）：
//! - `miniflux_token`（Miniflux API token）
//! - `ai_config`（AI 服务配置，含 api key）
//! - `config_sync_credentials`（Gist PAT / WebDAV 密码）
//!
//! 方案：Windows 上用 DPAPI（`CryptProtectData`，绑定当前用户，无需自管密钥），
//! 密文以 `dpapi:` 前缀 base64 存 SQLite；非 Windows 平台无 DPAPI，回落为
//! 明文 + 日志警示（FluxReader 目标平台为 Windows，非 Windows 仅开发态）。
//! 已有历史明文值在读时保持兼容（无 `dpapi:` 前缀的按明文原样返回），
//! 下次写入时自动升级为密文。

use crate::error::AppResult;
use rusqlite::OptionalExtension;

/// 密文前缀标记：以 `dpapi:` 开头说明是 DPAPI 密文，否则视为明文（历史遗留/非 Windows）。
const DPAPI_PREFIX: &str = "dpapi:";

/// 需要加密存储的 settings 键（SEC-2）。
pub const SENSITIVE_KEYS: &[&str] = &[
    "miniflux_token",
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
/// 本函数扫描这 3 个键，凡无 `dpapi:` 前缀且非空的，用 encrypt_secret 重写。
/// 幂等：已密文值跳过；每次启动都安全调用。
///
/// 非 Windows 平台无 DPAPI，encrypt_secret 只会回落明文（无法产生 `dpapi:`
/// 前缀），迁移永远无法"完成"——每次都重写相同的明文，幂等语义被破坏。
/// 故非 Windows 直接跳过迁移（返回 0），保持幂等（与 encrypt_secret 的回落
/// 行为一致：生产目标平台是 Windows，非 Windows 仅开发态，无需加密）。
pub fn migrate_legacy_plaintext(conn: &rusqlite::Connection) -> AppResult<usize> {
    #[cfg(not(windows))]
    {
        let _ = conn;
        return Ok(0);
    }
    #[cfg(windows)]
    {
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
                let enc = encrypt_secret(&raw);
                conn.execute(
                    "UPDATE settings SET value = ?1 WHERE key = ?2",
                    rusqlite::params![enc, key],
                )?;
                upgraded += 1;
            }
        }
        Ok(upgraded)
    }
}

/// 加密一段敏感值。成功返回 `dpapi:<base64>`；非 Windows 或失败时回退返回明文
/// （并在日志 warn），保证功能不因加密失败而中断。
pub fn encrypt_secret(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    if plain.starts_with(DPAPI_PREFIX) {
        // 已是密文，幂等
        return plain.to_string();
    }
    #[cfg(windows)]
    {
        match dpapi_encrypt(plain.as_bytes()) {
            Ok(cipher) => {
                use base64::Engine;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&cipher);
                format!("{DPAPI_PREFIX}{b64}")
            }
            Err(e) => {
                log::warn!("凭据 DPAPI 加密失败，回退明文存储: {e}");
                plain.to_string()
            }
        }
    }
    #[cfg(not(windows))]
    {
        log::warn!("非 Windows 平台无 DPAPI，凭据以明文存储（仅开发态，生产请用 Windows）");
        plain.to_string()
    }
}

/// 解密一段存库值。`dpapi:` 前缀走 DPAPI 解密；无前缀（历史明文/非 Windows）原样返回。
pub fn decrypt_secret(stored: &str) -> String {
    if stored.is_empty() {
        return String::new();
    }
    // `_b64` 前缀：Windows 分支会使用该绑定，非 Windows（Linux CI）分支不用——
    // 下划线前缀在未使用平台上抑制 unused_variables 警告（clippy -D warnings 下
    // 会导致 CI 失败），Windows 上被使用则无任何警告。
    if let Some(_b64) = stored.strip_prefix(DPAPI_PREFIX) {
        #[cfg(windows)]
        {
            use base64::Engine;
            match base64::engine::general_purpose::STANDARD.decode(_b64) {
                Ok(cipher) => match dpapi_decrypt(&cipher) {
                    Ok(plain) => return String::from_utf8_lossy(&plain).into_owned(),
                    Err(e) => {
                        log::warn!("凭据 DPAPI 解密失败，返回密文原文（可能已损坏）: {e}");
                        return stored.to_string();
                    }
                },
                Err(e) => {
                    log::warn!("凭据密文 base64 解析失败: {e}");
                    return stored.to_string();
                }
            }
        }
        #[cfg(not(windows))]
        {
            // 非 Windows 无法解密（不应出现 dpapi: 前缀，防御性返回）
            log::warn!("非 Windows 平台遇到 dpapi: 密文但无法解密");
            return stored.to_string();
        }
    }
    // 无前缀：历史明文或非 Windows 明文
    stored.to_string()
}

#[cfg(windows)]
fn dpapi_encrypt(plain: &[u8]) -> Result<Vec<u8>, String> {
    use windows::core::w;
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
    };

    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: plain.len() as u32,
        pbData: plain.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
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
        let bytes =
            std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        // 释放 DPAPI 分配的内存（HLOCAL 句柄）
        let _ = windows::Win32::Foundation::LocalFree(Some(
            windows::Win32::Foundation::HLOCAL(out_blob.pbData as *mut core::ffi::c_void),
        ));
        Ok(bytes)
    }
}

#[cfg(windows)]
fn dpapi_decrypt(cipher: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
    };

    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: cipher.len() as u32,
        pbData: cipher.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
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
        let bytes =
            std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        let _ = windows::Win32::Foundation::LocalFree(Some(
            windows::Win32::Foundation::HLOCAL(out_blob.pbData as *mut core::ffi::c_void),
        ));
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
        let enc = encrypt_secret(plain);
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
        let dec = decrypt_secret(&enc);
        assert_eq!(dec, plain, "解密往返应还原明文");
    }

    #[test]
    fn empty_and_legacy_plaintext_passthrough() {
        assert_eq!(encrypt_secret(""), "");
        // 历史明文（无前缀）读时原样返回，保证兼容
        assert_eq!(decrypt_secret("legacy-plain"), "legacy-plain");
    }

    #[test]
    fn idempotent_on_already_encrypted() {
        let enc = encrypt_secret("abc");
        let enc2 = encrypt_secret(&enc);
        assert_eq!(enc, enc2, "已密文值再加密应幂等");
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
            .query_row("SELECT value FROM settings WHERE key='miniflux_token'", [], |r| r.get(0))
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
}
