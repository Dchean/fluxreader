import { useEffect, useRef } from 'react';
import { useAppStore } from '../store';
import { api } from '../lib/api';

/* ============================================================
   播客底部播放条 —— Mini Player
   真实播放：单个 <audio> 元素随 isActive 挂载；store 状态 ↔ 元素双向同步。
   ============================================================ */

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

  const audioRef = useRef<HTMLAudioElement>(null);

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

  /* ---------- audio → store：进度/时长/结束 ---------- */
  useEffect(() => {
    const el = audioRef.current;
    if (!el) return;
    const onTime = () => syncPlayerProgress(el.currentTime, el.duration || 0);
    const onMeta = () => syncPlayerProgress(el.currentTime, el.duration || 0);
    const onEnd = () => playerEnded();
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
  }, [syncPlayerProgress, playerEnded, showToast, player.audioUrl]);

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

  /* 播放条卸载/关闭 → SMTC 停止 */
  useEffect(() => {
    if (!player.isActive && lastSentRef.current.title) {
      lastSentRef.current = { pos: -1, playing: null, title: '' };
      void api.mediaStop().catch(() => {});
    }
  }, [player.isActive]);

  if (!player.isActive) return null;

  const pct = player.durationSec > 0 ? Math.min(100, (player.positionSec / player.durationSec) * 100) : 0;

  return (
    <div className="podcast-bottom-bar active">
      {/* 单例 audio：随 isActive 挂载/卸载，src 变更即换剧集 */}
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
  );
}
