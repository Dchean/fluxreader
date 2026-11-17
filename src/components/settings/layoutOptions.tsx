import { LayoutIcon } from '../icons';
import { CONTENT_LAYOUTS, LAYOUT_NAMES } from '../../store';

/* P2-13：LAYOUT_OPTIONS 单点定义（设置页与 Overlays 浮层共用此份） */
export const LAYOUT_OPTIONS = CONTENT_LAYOUTS.map((l) => {
  const Icon = LayoutIcon[l];
  return { value: l as string, label: LAYOUT_NAMES[l], icon: <Icon /> };
});
