import { useCallback, useEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { Icons } from './icons';

/* ============================================================
   自定义 Mica 下拉选择框 —— 对应规范 §5.1 FluxDropdown
   菜单通过 Portal 挂载到 body 顶层（z-index 2000），
   绝不被任何 overflow 容器裁切；空间不足时自动向上弹出
   ============================================================ */

export interface DropdownOption {
  value: string;
  label: string;
  icon?: ReactNode;
}

interface FluxDropdownProps {
  options: DropdownOption[];
  value: string;
  onChange: (value: string) => void;
  width?: number | string;
}

export function FluxDropdown({ options, value, onChange, width = 160 }: FluxDropdownProps) {
  const [open, setOpen] = useState(false);
  const [menuStyle, setMenuStyle] = useState<CSSProperties>({});
  const triggerRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const updatePosition = useCallback(() => {
    const el = triggerRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const estHeight = Math.min(240, options.length * 32 + 10);
    const spaceBelow = window.innerHeight - rect.bottom;
    const spaceAbove = rect.top;
    /* 下方放不下且上方更宽敞（或按钮本身已在视口外）→ 向上弹 */
    const openUp = spaceBelow < estHeight && (spaceAbove >= estHeight || spaceAbove > spaceBelow);
    const left = Math.max(8, Math.min(rect.left, window.innerWidth - rect.width - 8));
    setMenuStyle(
      openUp
        ? { position: 'fixed', left, width: rect.width, minWidth: rect.width, bottom: Math.max(8, window.innerHeight - rect.top + 4), top: 'auto', zIndex: 2000, transform: 'none' }
        : { position: 'fixed', left, width: rect.width, minWidth: rect.width, top: rect.bottom + 4, bottom: 'auto', zIndex: 2000, transform: 'none' },
    );
  }, [options.length]);

  useEffect(() => {
    if (!open) return;
    updatePosition();
    const onDocClick = (e: MouseEvent) => {
      const t = e.target as Node;
      if (triggerRef.current?.contains(t) || menuRef.current?.contains(t)) return;
      setOpen(false);
    };
    const onEsc = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      e.stopPropagation();
      setOpen(false);
    };
    const onReflow = () => updatePosition();
    window.addEventListener('click', onDocClick, true);
    document.addEventListener('keydown', onEsc, true);
    /* capture: 捕获弹窗内部滚动容器的滚动（settings-content-pane 等），菜单实时跟随 */
    window.addEventListener('scroll', onReflow, true);
    window.addEventListener('resize', onReflow);
    return () => {
      window.removeEventListener('click', onDocClick, true);
      document.removeEventListener('keydown', onEsc, true);
      window.removeEventListener('scroll', onReflow, true);
      window.removeEventListener('resize', onReflow);
    };
  }, [open, updatePosition]);

  const current = options.find((o) => o.value === value) ?? options[0];

  return (
    <div className={`flux-dropdown ${open ? 'open' : ''}`} ref={triggerRef} style={{ width }}>
      <div className="flux-dropdown-trigger" onClick={() => setOpen((v) => !v)}>
        <span className="flux-dropdown-label">{current?.label}</span>
        <svg className="svg-icon chevron-svg" viewBox="0 0 24 24"><polyline points="6 9 12 15 18 9"/></svg>
      </div>
      {open &&
        createPortal(
          <div className="flux-dropdown-menu open" style={menuStyle} ref={menuRef}>
            {options.map((opt) => (
              <div
                key={opt.value}
                className={`flux-dropdown-option ${opt.value === value ? 'active' : ''}`}
                onClick={() => {
                  onChange(opt.value);
                  setOpen(false);
                }}
              >
                <div className="opt-label-group">
                  {opt.icon}
                  <span>{opt.label}</span>
                </div>
                <svg className="svg-icon opt-check" viewBox="0 0 24 24"><polyline points="20 6 9 17 4 12"/></svg>
              </div>
            ))}
          </div>,
          document.body,
        )}
    </div>
  );
}

/* ============================================================
   开关控件
   ============================================================ */

interface SwitchProps {
  checked: boolean;
  onChange: (val: boolean) => void;
  id?: string;
}

export function Switch({ checked, onChange, id }: SwitchProps) {
  return (
    <label className="switch-control">
      <input
        type="checkbox"
        id={id}
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="switch-slider" />
    </label>
  );
}

/* ============================================================
   设置卡片
   ============================================================ */

interface SettingCardProps {
  title: string;
  desc?: string;
  children?: ReactNode;
}

export function SettingCard({ title, desc, children }: SettingCardProps) {
  return (
    <div className="setting-card">
      {/* 左列文字允许收缩（min-width:0 在 CSS），控件列 flex-shrink:0 —— 窄窗口下文字不再被压到一字一行 */}
      <div className="setting-card-text">
        <h5>{title}</h5>
        {desc && <p>{desc}</p>}
      </div>
      {children}
    </div>
  );
}

/* ============================================================
   二次确认弹窗 —— 破坏性操作的统一确认原语
   挂在最高层（z-index 300，高于设置弹窗 150 / 下拉菜单 2000 内
   仍置于顶层容器），danger 风格按钮，Esc/遮罩点击 = 取消
   ============================================================ */

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  message: string;
  confirmText?: string;
  cancelText?: string;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  open,
  title,
  message,
  confirmText = '删除',
  cancelText = '取消',
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  useEffect(() => {
    if (!open) return;
    const onEsc = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        e.preventDefault();
        onCancel();
      }
    };
    document.addEventListener('keydown', onEsc, true);
    return () => document.removeEventListener('keydown', onEsc, true);
  }, [open, onCancel]);

  if (!open) return null;

  return createPortal(
    <div className="modal-overlay confirm-overlay open" onClick={(e) => { if (e.target === e.currentTarget) onCancel(); }}>
      <div className="mini-dialog confirm-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="confirm-icon-badge"><Icons.trash /></div>
        <div className="confirm-title">{title}</div>
        <div className="confirm-message">{message}</div>
        <div className="mini-dialog-actions">
          <button className="toggle-action-btn" onClick={onCancel}>{cancelText}</button>
          <button className="toggle-action-btn btn-danger" onClick={onConfirm}>
            <Icons.trash />
            <span>{confirmText}</span>
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

interface ModalOverlayProps {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  contentWidth?: number | string;
}

export function ModalOverlay({ open, onClose, children, contentWidth }: ModalOverlayProps) {
  return (
    <div
      className={`modal-overlay ${open ? 'open' : ''}`}
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={
          contentWidth
            ? { width: contentWidth, background: 'var(--bg-elevated)', border: '1px solid var(--border-strong)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-elevation)', overflow: 'hidden' }
            : undefined
        }
      >
        {children}
      </div>
    </div>
  );
}
