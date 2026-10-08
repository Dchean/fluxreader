import type { StateCreator } from 'zustand';
import type { AppState, MediaAction } from '../types';

/** 播客播放器 slice：PlayerBar 状态机（播放/暂停/进度回写/倍速/展开态）。
 *
 *  Pick 的键集即本 slice 的全部键；与其它 slice 两两不相交（合起来 = 原 useAppStore 全集）。
 */
export type PlayerSlice = Pick<
  AppState,
  | 'player'
  | 'playerExpanded'
  | 'playPodcastEpisode'
  | 'togglePlayerPlay'
  | 'applyMediaAction'
  | 'syncPlayerProgress'
  | 'playerEnded'
  | 'seekPlayer'
  | 'skipPlayer'
  | 'cyclePlaybackSpeed'
  | 'closePodcastBar'
  | 'togglePlayerExpanded'
>;

export const createPlayerSlice: StateCreator<AppState, [], [], PlayerSlice> = (set, get) => ({
  player: { isActive: false, isPlaying: false, speed: 1.0, title: '', showName: '', cover: '', coverEntryId: '', audioUrl: '', positionSec: 0, durationSec: 0, seekToSec: null },
  playerExpanded: false,

  /* ================= 播客 ================= */

  /** 播放真实剧集：audioUrl 必传（enclosure_url），PlayerBar 挂 audio 元素执行。 */
  playPodcastEpisode: (title, showName, cover, audioUrl, entryId) => {
    if (!audioUrl) {
      get().showToast('该剧集没有可播放的音频地址');
      return;
    }
    set({
      player: {
        ...get().player,
        isActive: true,
        isPlaying: true,
        title,
        showName,
        cover: cover || get().player.cover,
        coverEntryId: cover ? (entryId ?? '') : get().player.coverEntryId,
        audioUrl,
        positionSec: 0,
        durationSec: 0,
        seekToSec: null,
      },
    });
    get().showToast(`正在播放：${title}`);
    /* 点播放即视为已读（与打开文章同语义） */
    if (entryId) get().markEntriesReadBulk([entryId]);
  },

  togglePlayerPlay: () =>
    set((s) => ({ player: { ...s.player, isPlaying: !s.player.isPlaying } })),

  /* OPT-015：媒体键动作幂等收口（App.tsx player-media 事件唯一落点）。
     此前 App 对 play/pause/toggle 一律调 togglePlayerPlay——系统按「播放」
     而当前已在播放时会反被暂停（用户实测的 Play/Pause 翻转缺陷）。这里按
     动作语义区分：play/pause 是**目标态**（已在目标态 = no-op，不产生新
     对象、不触发多余重渲染），toggle 才承担切换；未激活（无剧集）时
     play/pause/toggle 全为 no-op——媒体键不得凭空启动无源播放条；
     stop 仍走既有 closePodcastBar（关闭语义单点，幂等无害）。
     幂等 ≠ 失效：暂停后按 Play 会置回 isPlaying=true 从暂停点恢复；
     音频元素 play() 失败的回退（PlayerBar 翻回暂停 + toast）保留，
     再按 Play 仍是一次真实状态变化，会重新驱动元素重试。
     Note: 语义与证据边界见 .agents/notes/implemented/bug-fix/2026-10-08-窗口首帧恢复与媒体命令幂等.md */
  applyMediaAction: (action: MediaAction) => {
    if (action === 'stop') {
      get().closePodcastBar();
      return;
    }
    const p = get().player;
    if (!p.isActive) return;
    if (action === 'play') {
      if (!p.isPlaying) set({ player: { ...p, isPlaying: true } });
    } else if (action === 'pause') {
      if (p.isPlaying) set({ player: { ...p, isPlaying: false } });
    } else {
      set({ player: { ...p, isPlaying: !p.isPlaying } });
    }
  },

  /** audio 元素状态回写（timeupdate/loadedmetadata/durationchange 调） */
  syncPlayerProgress: (positionSec, durationSec) =>
    set((s) => ({ player: { ...s.player, positionSec, durationSec } })),

  /** 播放自然结束（ended 事件） */
  playerEnded: () =>
    set((s) => ({ player: { ...s.player, isPlaying: false, positionSec: 0, seekToSec: null } })),

  seekPlayer: (sec) =>
    set((s) => ({ player: { ...s.player, seekToSec: Math.max(0, sec), positionSec: Math.max(0, sec) } })),

  skipPlayer: (deltaSec) => {
    const p = get().player;
    set({ player: { ...p, seekToSec: Math.max(0, p.positionSec + deltaSec), positionSec: Math.max(0, p.positionSec + deltaSec) } });
  },

  cyclePlaybackSpeed: () => {
    const speeds = [1.0, 1.25, 1.5, 2.0];
    const next = speeds[(speeds.indexOf(get().player.speed) + 1) % speeds.length];
    set((s) => ({ player: { ...s.player, speed: next } }));
    get().showToast(`倍速已切换至 ${next}x`);
  },

  closePodcastBar: () =>
    set((s) => ({ player: { ...s.player, isActive: false, isPlaying: false, seekToSec: null }, playerExpanded: false })),

  togglePlayerExpanded: () => set((s) => ({ playerExpanded: !s.playerExpanded })),
});
