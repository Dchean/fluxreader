import { useEffect, useRef } from 'react';
import { useAppStore } from '../store';
import { api } from '../lib/api';

/* ============================================================
   播客底部播放条 —— Mini Player
   真实播放：单个 <audio> 元素随 isActive 挂载；store 状态 ↔ 元素双向同步。
   续播记忆：进度节流落库 settings(last_playback)；重开同一集从上次位置续播。
   ============================================================ */

/** 续播记录 settings 键（含 url/title/show/cover/pos/duration/updatedAt） */
const PLAYBACK_KEY = 'last_playback';

function persistPlayback(p: {
  url: string; title: string; show: string; cover: string; positionSec: number; durationSec: number;
}) {
  // 只在 Tauri 环境落库（浏览器 mock 无 IPC）
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
  void api.setSetting(PLAYBACK_KEY, JSON.stringify({
    url: p.url, title: p.title, show: p.show, cover: p.cover,
    positionSec: p.positionSec, durationSec: p.durationSec, updatedAt: Date.now(),
  })).catch(() => {/* 落库失败不影响播放 */ });
}

function clearPlayback() {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
  void api.setSetting(PLAYBACK_KEY, '').catch(() => {});
}

function formatClock(sec: number): string {
  if (!Number.isFinite(sec) || sec < 0) return '0:00';
  const m = Math.floor(sec / 60);
  const s = Math.floor(sec % 60);
  return `${m}:${s.toString().padStart(2, '0')}`;
}

export function PlayerBar() {
  const player = useAppStore((s) => s.player);
  const togglePlayerPlay = useAppStore((s) => s.togglePlayerPlay);
  const cyclePlaybackSpeed = useAppStore((s) => s.cyclePlaybackSpeed);
  const closePodcastBar = useAppStore((s) => s.closePodcastBar);
  const syncPlayerProgress = useAppStore((s) => s.syncPlayerProgress);
  const playerEnded = useAppStore((s) => s.playerEnded);
  const seekPlayer = useAppStore((s) => s.seekPlayer);
  const skipPlayer = useAppStore((s) => s.skipPlayer);
  const showToast = useAppStore((s) => s.showToast);
  const playerExpanded = useAppStore((s) => s.playerExpanded);
  const togglePlayerExpanded = useAppStore((s) => s.togglePlayerExpanded);

  const audioRef = useRef<HTMLAudioElement>(null);
  /** 待续播位置（读自 last_playback）：{url,pos}，与当前 audioUrl 命中时 seek 一次 */
  const pendingResumeRef = useRef<{ url: string; pos: number } | null>(null);
  /** 落库节流：记录上次写入的整秒 */
  const lastSaveRef = useRef(-1);

  /* 换剧集时读续播记录：命中同 URL 且未播完 → 记录待 seek */
  useEffect(() => {
    if (!player.isActive || !player.audioUrl) return;
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    let alive = true;
    lastSaveRef.current = -1;
    pendingResumeRef.current = null;
    void api.getSetting(PLAYBACK_KEY).then((raw) => {
      if (!alive || !raw) return;
      try {
        const r = JSON.parse(raw) as { url?: string; positionSec?: number; durationSec?: number };
        const pos = Number(r.positionSec) || 0;
        const dur = Number(r.durationSec) || 0;
        // 命中同一集、有有效位置、且未接近结束（< 时长-5s）→ 续播
        if (r.url === player.audioUrl && pos > 0 && (dur <= 0 || pos < dur - 5)) {
          pendingResumeRef.current = { url: r.url, pos };
        }
      } catch { /* 坏记录忽略 */ }
    });
    return () => { alive = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [player.audioUrl, player.isActive]);

  /* ---------- store → audio：播放/暂停/倍速/seek ---------- */
  useEffect(() => {
    const el = audioRef.current;
    if (!el) return;
    if (player.isPlaying) void el.play().catch(() => {
      /* 自动播放被拒或网络失败：回落暂停态并提示 */
      useAppStore.getState().togglePlayerPlay();
      useAppStore.getState().showToast('音频加载失败');
    });
    else el.pause();
  }, [player.isPlaying, player.audioUrl]);

  useEffect(() => {
    if (audioRef.current) audioRef.current.playbackRate = player.speed;
  }, [player.speed, player.audioUrl]);

  /* seek 请求（seekToSec 非 null 时执行一次并清空） */
  useEffect(() => {
    const el = audioRef.current;
    if (!el || player.seekToSec == null) return;
    el.currentTime = player.seekToSec;
    useAppStore.setState((s) => ({ player: { ...s.player, seekToSec: null } }));
  }, [player.seekToSec]);

  /* ---------- audio → store：进度/时长/结束 + 续播 seek/落库 ---------- */
  useEffect(() => {
    const el = audioRef.current;
    if (!el) return;
    const onMeta = () => {
      syncPlayerProgress(el.currentTime, el.duration || 0);
      // 续播：元数据就绪后再 seek（此时可安全设 currentTime）
      const pr = pendingResumeRef.current;
      if (pr && pr.url === player.audioUrl && pr.pos > 0) {
        el.currentTime = pr.pos;
        syncPlayerProgress(pr.pos, el.duration || 0);
        pendingResumeRef.current = null;
      }
    };
    const onTime = () => {
      syncPlayerProgress(el.currentTime, el.duration || 0);
      // 节流落库（整秒变化才写，避免每帧写）
      const s = Math.floor(el.currentTime);
      if (s !== lastSaveRef.current) {
        lastSaveRef.current = s;
        if (s > 0 && el.duration && el.currentTime < el.duration - 3) {
          persistPlayback({
            url: player.audioUrl, title: player.title, show: player.showName,
            cover: player.cover || '', positionSec: el.currentTime, durationSec: el.duration,
          });
        }
      }
    };
    const onEnd = () => {
      playerEnded();
      clearPlayback();
    };
    const onError = () => showToast('音频播放出错');
    el.addEventListener('timeupdate', onTime);
    el.addEventListener('loadedmetadata', onMeta);
    el.addEventListener('durationchange', onMeta);
    el.addEventListener('ended', onEnd);
    el.addEventListener('error', onError);
    return () => {
      el.removeEventListener('timeupdate', onTime);
      el.removeEventListener('loadedmetadata', onMeta);
      el.removeEventListener('durationchange', onMeta);
      el.removeEventListener('ended', onEnd);
      el.removeEventListener('error', onError);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [player.audioUrl, player.title, player.showName, player.cover]);

  /* ---------- SMTC 系统媒体控制同步（节流 1s，进度变化才推送） ---------- */
  const lastSentRef = useRef({ pos: -1, playing: null as boolean | null, title: '' });
  useEffect(() => {
    // 播放条关闭 → SMTC 置 Stopped
    if (!player.isActive) return;
    const pos = Math.floor(player.positionSec);
    const metaChanged = player.title !== lastSentRef.current.title;
    const posChanged = pos !== lastSentRef.current.pos;
    const stateChanged = player.isPlaying !== lastSentRef.current.playing;
    if (!metaChanged && !posChanged && !stateChanged) return;
    lastSentRef.current = { pos, playing: player.isPlaying, title: player.title };
    void api.mediaUpdateFull(
      player.title,
      player.showName,
      player.durationSec,
      player.positionSec,
      player.isPlaying,
    ).catch(() => {/* SMTC 失败不影响播放 */ });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [player.title, player.showName, player.positionSec, player.isPlaying, player.durationSec, player.isActive]);

  /* 播放条卸载/关闭 → 保存进度 + SMTC 停止 */
  useEffect(() => {
    if (!player.isActive) {
      if (lastSentRef.current.title) {
        // 关闭前把当前进度落库（未播完才存）
        if (player.audioUrl && player.positionSec > 0 && (player.durationSec <= 0 || player.positionSec < player.durationSec - 3)) {
          persistPlayback({
            url: player.audioUrl, title: player.title, show: player.showName,
            cover: player.cover || '', positionSec: player.positionSec, durationSec: player.durationSec,
          });
        }
        lastSentRef.current = { pos: -1, playing: null, title: '' };
        void api.mediaStop().catch(() => {});
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [player.isActive]);

  if (!player.isActive) return null;

  const pct = player.durationSec > 0 ? Math.min(100, (player.positionSec / player.durationSec) * 100) : 0;

  return (
    <>
      <div className="podcast-bottom-bar active">
        {/* 单例 audio：随 isActive 挂载/卸载，src 变更即换剧集。续播 seek 在 onMeta 做。 */}
        <audio ref={audioRef} src={player.audioUrl} preload="metadata" />

        <div className="player-track-info">
          {player.cover && <img src={player.cover} className="player-cover" alt="cover" referrerPolicy="no-referrer" />}
          <div className="player-titles">
            <div className="player-title">{player.title}</div>
            <div className="player-subtitle">{player.showName}</div>
          </div>
        </div>

        <div className="player-center-controls">
          <div className="player-buttons-row">
            <button className="win-btn" onClick={() => skipPlayer(-15)} title="后退 15 秒">↺ 15</button>
            <button
              className="podcast-play-circle"
              onClick={togglePlayerPlay}
              style={{ width: 32, height: 32, fontSize: 11 }}
              title={player.isPlaying ? '暂停 (Space)' : '播放 (Space)'}
            >
              {player.isPlaying ? '⏸' : '▶'}
            </button>
            <button className="win-btn" onClick={() => skipPlayer(30)} title="快进 30 秒">30 ↻</button>
          </div>
          <div className="player-progress-row">
            <span className="player-time-tag">{formatClock(player.positionSec)}</span>
            <div
              className="player-progress-track"
              onClick={(e) => {
                if (player.durationSec <= 0) return;
                const rect = e.currentTarget.getBoundingClientRect();
                const ratio = (e.clientX - rect.left) / rect.width;
                seekPlayer(Math.max(0, Math.min(1, ratio)) * player.durationSec);
              }}
              role="slider"
              aria-valuenow={Math.round(pct)}
              aria-valuemin={0}
              aria-valuemax={100}
              tabIndex={0}
            >
              <div className="player-progress-fill" style={{ width: `${pct}%` }} />
            </div>
            <span className="player-time-tag">{formatClock(player.durationSec)}</span>
          </div>
        </div>

        <div className="player-right-controls">
          <button className="toggle-action-btn" onClick={togglePlayerExpanded} title="展开全屏播放器"
            style={{ padding: '2px 8px', fontSize: 11 }}>⛶ 展开</button>
          <button
            className="toggle-action-btn"
            onClick={cyclePlaybackSpeed}
            style={{ padding: '2px 8px', fontSize: 11 }}
          >
            {player.speed.toFixed(1)}x
          </button>
          <button className="win-btn" onClick={closePodcastBar} title="关闭播放器">✕</button>
        </div>
      </div>

      {/* Full Player 大浮层：复用同一 store 播放源（audio 在 Mini 常驻），仅提供更大视图与控制 */}
      {playerExpanded && (
        <PlayerFullOverlay
          player={player}
          onPlayPause={togglePlayerPlay}
          onSeek={seekPlayer}
          onSkip={skipPlayer}
          onSpeed={cyclePlaybackSpeed}
          onClose={() => togglePlayerExpanded()}
          onStop={closePodcastBar}
          formatClock={formatClock}
        />
      )}
    </>
  );
}

/** Full Player 覆盖层：封面/标题/大进度条/控制钮；无独立 audio（复用底部条的音源）。 */
function PlayerFullOverlay({
  player, onPlayPause, onSeek, onSkip, onSpeed, onClose, onStop, formatClock,
}: {
  player: ReturnType<typeof useAppStore.getState>['player'];
  onPlayPause: () => void;
  onSeek: (sec: number) => void;
  onSkip: (d: number) => void;
  onSpeed: () => void;
  onClose: () => void;
  onStop: () => void;
  formatClock: (sec: number) => string;
}) {
  const pct = player.durationSec > 0 ? Math.min(100, (player.positionSec / player.durationSec) * 100) : 0;
  const seekByRatio = (clientX: number, el: HTMLElement) => {
    if (player.durationSec <= 0) return;
    const rect = el.getBoundingClientRect();
    const ratio = (clientX - rect.left) / rect.width;
    onSeek(Math.max(0, Math.min(1, ratio)) * player.durationSec);
  };
  return (
    <div className="player-full-overlay" role="dialog" aria-modal="true" onClick={onClose}>
      <div className="player-full-card" onClick={(e) => e.stopPropagation()}>
        <div className="player-full-cover-wrap">
          {player.cover
            ? <img src={player.cover} className="player-full-cover" alt="" referrerPolicy="no-referrer" />
            : <div className="player-full-cover player-full-cover-fallback" />}
        </div>
        <div className="player-full-meta">
          <div className="player-full-show">{player.showName}</div>
          <div className="player-full-title">{player.title}</div>
          <div className="player-full-progress">
            <div className="player-progress-track player-full-track"
              onClick={(e) => seekByRatio(e.clientX, e.currentTarget)}>
              <div className="player-progress-fill" style={{ width: `${pct}%` }} />
            </div>
            <div className="player-full-times">
              <span>{formatClock(player.positionSec)}</span>
              <span>{formatClock(player.durationSec)}</span>
            </div>
          </div>
          <div className="player-full-controls">
            <button className="win-btn" onClick={() => onSkip(-15)} title="后退 15 秒">↺ 15</button>
            <button className="podcast-play-circle player-full-play" onClick={onPlayPause}
              title={player.isPlaying ? '暂停 (Space)' : '播放 (Space)'}>
              {player.isPlaying ? '⏸' : '▶'}
            </button>
            <button className="win-btn" onClick={() => onSkip(30)} title="快进 30 秒">30 ↻</button>
            <button className="toggle-action-btn" onClick={onSpeed} title="倍速">{player.speed.toFixed(1)}x</button>
          </div>
          <div className="player-full-actions">
            <button className="toggle-action-btn" onClick={onClose}>收起</button>
            <button className="toggle-action-btn" onClick={onStop}>关闭播放器</button>
          </div>
        </div>
      </div>
    </div>
  );
}
