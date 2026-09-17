import type { StateCreator } from 'zustand';
import type { AppState } from '../types';

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
  | 'syncPlayerProgress'
  | 'playerEnded'
  | 'seekPlayer'
  | 'skipPlayer'
  | 'cyclePlaybackSpeed'
  | 'closePodcastBar'
  | 'togglePlayerExpanded'
>;

export const createPlayerSlice: StateCreator<AppState, [], [], PlayerSlice> = (set, get) => ({
  player: { isActive: false, isPlaying: false, speed: 1.0, title: '', showName: '', cover: '', audioUrl: '', positionSec: 0, durationSec: 0, seekToSec: null },
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
