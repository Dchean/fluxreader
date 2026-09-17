import { LayoutIcon } from '../icons';
import { CONTENT_LAYOUTS, LAYOUT_NAMES } from '../../store';

export const LAYOUT_OPTIONS = CONTENT_LAYOUTS.map((l) => {
  const Icon = LayoutIcon[l];
  return { value: l as string, label: LAYOUT_NAMES[l], icon: <Icon /> };
});
