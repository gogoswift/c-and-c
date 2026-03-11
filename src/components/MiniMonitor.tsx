/**
 * MiniMonitor — 精灵动画猫 迷你悬浮监控窗口 (180x140)
 *
 * 使用 cat-sprite.png 精灵表 (32x32 每帧)
 * 行定义:
 *   0: 坐1 (4帧)  1: 坐2 (4帧)  2: 坐3 (4帧)  3: 坐4 (4帧)
 *   4: 跑1 (8帧)  5: 跑2 (8帧)
 *   6: 睡  (4帧)
 *   7: 玩  (6帧)  8: 扑  (7帧)  9: 伸懒腰 (8帧)
 *
 * 没有活跃智能体 → 睡动画
 * 有活跃智能体 → 每只猫随机分配非睡动画
 */

import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import catSpriteUrl from '../assets/cat-sprite.png';
import cat2SpriteUrl from '../assets/cat2-sprite.png';

// ═══════════════════════════════════════════════════════════════
//  类型定义（原 @agent-devtools/core 和 electron.d.ts 中的类型）
// ═══════════════════════════════════════════════════════════════

interface ToolCall {
  startTime: Date | null;
  endTime: Date | null;
  isError: boolean;
  [key: string]: unknown;
}

interface Turn {
  toolCalls: ToolCall[];
  [key: string]: unknown;
}

interface SubAgentRef {
  session?: Session | null;
  [key: string]: unknown;
}

interface Session {
  turns: Turn[];
  subAgents?: SubAgentRef[];
  tokenUsage: { estimatedCost: number; [key: string]: unknown };
  [key: string]: unknown;
}

interface UsageTier {
  utilization: number;
  resets_at: string | null;
}

interface ClaudeUsage {
  five_hour: UsageTier;
  seven_day: UsageTier;
  seven_day_opus?: UsageTier;
  seven_day_sonnet?: UsageTier;
}

interface ClaudeUserInfo {
  userType: 'not_logged_in' | 'api_user' | 'subscriber';
  subscriptionType: string | null;
  accountType: string | null;
  usage: ClaudeUsage | null;
  error: string | null;
}

interface CodexRateLimitTier {
  used_percent: number;
  window_minutes: number;
  resets_at: number; // Unix 秒级时间戳
}

interface CodexRateLimits {
  primary: CodexRateLimitTier;
  secondary: CodexRateLimitTier;
  plan_type: string | null;
  source: 'api' | 'jsonl';
}

interface ActiveSessionInfo {
  sessionId: string;
  projectHash: string;
  filePath: string;
  lastModified: string;
  subAgentFiles: string[];
  title: string | null;
  slug: string | null;
  cwd: string | null;
}

interface ActiveCodexSessionInfo {
  sessionId: string;
  filePath: string;
  lastModified: string;
  cwd: string | null;
  title: string | null;
  startTime: string | null;
  hasActiveTool: boolean;
  hasError: boolean;
  toolCount: number;
  totalTokens: number;
}

// ═══════════════════════════════════════════════════════════════
//  精灵表配置
// ═══════════════════════════════════════════════════════════════

const FRAME_SIZE = 32;    // 每帧 32x32 像素
// 精灵裁切：实际猫像素范围 x=[7,24] y=[14,31]，即 18×18
const CROP_X = 7;         // 裁切起始 x（原始像素）
const CROP_Y = 14;        // 裁切起始 y（原始像素）
const CROP_W = 18;        // 裁切宽度（原始像素）
const CROP_H = 18;        // 裁切高度（原始像素）
const SCALE = 75 / FRAME_SIZE; // 缩放比 ≈ 2.34
const DISPLAY_W = Math.round(CROP_W * SCALE); // 裁切后显示宽度 ≈ 42
const DISPLAY_H = Math.round(CROP_H * SCALE); // 裁切后显示高度 ≈ 42
const DISPLAY_SIZE = Math.max(DISPLAY_W, DISPLAY_H); // 兼容旧引用

// 动画定义: [行号, 帧数]
const ANIMS = {
  sit1:    { row: 0, frames: 4 },
  sit2:    { row: 1, frames: 4 },
  sit3:    { row: 2, frames: 4 },
  sit4:    { row: 3, frames: 4 },
  run1:    { row: 4, frames: 8 },
  run2:    { row: 5, frames: 8 },
  sleep:   { row: 6, frames: 4 },
  play:    { row: 7, frames: 6 },
  pounce:  { row: 8, frames: 7 },
  stretch: { row: 9, frames: 8 },
} as const;

type AnimName = keyof typeof ANIMS;

// 所有动画名
const ALL_ANIM_NAMES: AnimName[] = Object.keys(ANIMS) as AnimName[];

// 跑步动画（用于碰撞后逃跑）
const RUN_ANIMS: AnimName[] = ['run1', 'run2'];
// 碰撞检测阈值：两猫左边缘距离小于此值视为相交（约半个精灵宽度）
const OVERLAP_THRESHOLD = Math.round(DISPLAY_W * 0.5);

// 需要移动的动画及每帧移动像素数（正数 = 移动中）
const MOVING_ANIMS: Set<AnimName> = new Set(['run1', 'run2', 'pounce']);
const MOVE_PX: Partial<Record<AnimName, number>> = {
  run1: 3, run2: 3, pounce: 1.5,
};
const WINDOW_W = 180;
const CAT_MARGIN = 10; // 左右留白

// 真实猫毛色 — 用 CSS filter 组合模拟
// 原始精灵是黑白猫，通过 filter 叠加实现不同毛色
const CAT_COLORS: { name: string; filter: string }[] = [
  { name: '黑白猫', filter: '' },                                                              // 原色
  { name: '黑猫',   filter: 'brightness(0.25) contrast(1.4)' },                                // 深黑
  { name: '白猫',   filter: 'brightness(1.6) contrast(0.6) saturate(0)' },                     // 亮白灰
  { name: '蓝猫',   filter: 'brightness(0.75) contrast(1.1) saturate(0.3) hue-rotate(200deg)' }, // 蓝灰
  { name: '橘猫',   filter: 'sepia(0.8) saturate(2.5) hue-rotate(-10deg) brightness(1.1)' },    // 橘黄
  { name: '狸猫',   filter: 'sepia(0.6) saturate(1.2) hue-rotate(10deg) brightness(0.85)' },    // 棕褐
  { name: '奶牛猫', filter: 'brightness(0.4) contrast(2.0) saturate(0)' },                      // 黑白高对比
  { name: '暹罗猫', filter: 'sepia(0.3) saturate(0.8) brightness(1.2) contrast(0.9)' },         // 米黄
];

// ═══════════════════════════════════════════════════════════════
//  日夜主题 — 根据电脑时间切换
// ═══════════════════════════════════════════════════════════════

function getTimeOfDay(): 'night' | 'dawn' | 'day' | 'dusk' {
  const h = new Date().getHours();
  if (h >= 6 && h < 8) return 'dawn';
  if (h >= 8 && h < 17) return 'day';
  if (h >= 17 && h < 19) return 'dusk';
  return 'night';
}

const THEMES = {
  night: {
    sky: '#12121e',
    titleBar: '#0e0e1a',
    border: '#2a2a3e',
    statusBar: '#0e0e1a',
    starColor: '#334466',
    showStars: true,
    groundTop: '#2a3a2a',
    groundBot: '#1a2a1a',
    grassA: '#3a5a3a',
    grassB: '#2a4a2a',
  },
  dawn: {
    sky: 'linear-gradient(to bottom, #2a1a3a, #4a3060, #c87040)',
    titleBar: '#2a1a30',
    border: '#4a3060',
    statusBar: '#2a1a30',
    starColor: '#665588',
    showStars: true,
    groundTop: '#3a4a2a',
    groundBot: '#2a3a1a',
    grassA: '#4a6a3a',
    grassB: '#3a5a2a',
  },
  day: {
    sky: 'linear-gradient(to bottom, #5588cc, #88bbee)',
    titleBar: '#4477aa',
    border: '#6699cc',
    statusBar: '#4477aa',
    starColor: '',
    showStars: false,
    groundTop: '#5a8a3a',
    groundBot: '#4a7a2a',
    grassA: '#6aaa4a',
    grassB: '#5a9a3a',
  },
  dusk: {
    sky: 'linear-gradient(to bottom, #3a4488, #885544, #cc7744)',
    titleBar: '#2a3366',
    border: '#4a5588',
    statusBar: '#2a3366',
    starColor: '#556688',
    showStars: true,
    groundTop: '#3a5a2a',
    groundBot: '#2a4a1a',
    grassA: '#4a7a3a',
    grassB: '#3a6a2a',
  },
} as const;

// ═══════════════════════════════════════════════════════════════
//  工具函数
// ═══════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════
//  迷你用量进度条（订阅用户用）
// ═══════════════════════════════════════════════════════════════

/** 格式化重置时间为相对时间（如 "2h30m" / "3d"） */
function formatResetTime(resetAt: string | null): string | null {
  if (!resetAt) return null;
  const diff = new Date(resetAt).getTime() - Date.now();
  if (diff <= 0) return null;
  const m = Math.floor(diff / 60_000);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h${m % 60 > 0 ? `${m % 60}m` : ''}`;
  const d = Math.floor(h / 24);
  return `${d}d${h % 24 > 0 ? `${h % 24}h` : ''}`;
}

/**
 * 内联用量标签：label pct% (无进度条)
 */
// cc/cx 品牌色
const BRAND_COLORS: Record<string, string> = {
  cc: '#d4a574',  // 暖杏色（Claude）
  cx: '#74AA9C',  // 青绿色（Codex）
};

function UsageTag({ label, pct }: {
  label: string; pct: number;
}) {
  const pctColor = pct > 80 ? '#ff6b6b' : pct > 50 ? '#ffd43b' : '#a6e3a1';
  const dotIdx = label.indexOf('·');
  const prefix = dotIdx > 0 ? label.slice(0, dotIdx) : label;
  const suffix = dotIdx > 0 ? label.slice(dotIdx) : '';
  const brandColor = BRAND_COLORS[prefix] ?? '#aaa';
  return (
    <span style={{
      display: 'inline-flex', alignItems: 'baseline', gap: 2,
      fontFamily: 'monospace', fontWeight: 700,
      fontSize: 8, lineHeight: 1, whiteSpace: 'nowrap',
    }}>
      <span>
        <span style={{ color: brandColor }}>{prefix}</span>
        <span style={{ color: '#888' }}>{suffix}</span>
      </span>
      <span style={{ color: pctColor }}>{pct.toFixed(0)}%</span>
    </span>
  );
}

// ═══════════════════════════════════════════════════════════════
//  右侧滑出面板 — 详细额度信息
// ═══════════════════════════════════════════════════════════════

interface UsageRow {
  label: string;
  pct: number;
  color: string;
  resetAt: string | null;
}

function UsageDetailPanel({ type, rows, onClose }: {
  type: 'cc' | 'cx';
  rows: UsageRow[];
  onClose: () => void;
}) {
  const brandColor = BRAND_COLORS[type] ?? '#aaa';
  const title = type === 'cc' ? 'Claude Code' : 'Codex';

  return (
    <>
      {/* 半透明遮罩 */}
      <div
        onClick={onClose}
        style={{
          position: 'absolute', inset: 0, zIndex: 10,
          background: 'rgba(0,0,0,0.3)',
        }}
      />
      {/* 面板 — 从右侧滑入 */}
      <div style={{
        position: 'absolute', top: 0, right: 0, bottom: 0,
        width: 130, zIndex: 11,
        background: 'rgba(15,15,25,0.95)',
        borderLeft: `1px solid ${brandColor}44`,
        backdropFilter: 'blur(8px)',
        display: 'flex', flexDirection: 'column',
        animation: 'slideInRight 0.2s ease-out',
        overflow: 'hidden',
      }}>
        {/* 标题 */}
        <div style={{
          padding: '4px 8px 3px',
          borderBottom: `1px solid ${brandColor}33`,
          display: 'flex', alignItems: 'center', justifyContent: 'space-between',
        }}>
          <span style={{
            fontSize: 10, fontWeight: 900, fontFamily: 'monospace',
            color: brandColor,
          }}>{title}</span>
          <span
            onClick={onClose}
            style={{
              fontSize: 10, color: '#666', cursor: 'pointer',
              lineHeight: 1, padding: '0 2px',
            }}
          >✕</span>
        </div>
        {/* 额度条列表 */}
        <div style={{
          padding: '4px 8px',
          display: 'flex', flexDirection: 'column', gap: 5,
        }}>
          {rows.map((row, i) => {
            const pctColor = row.pct > 80 ? '#ff6b6b' : row.pct > 50 ? '#ffd43b' : '#a6e3a1';
            const resetStr = row.resetAt ? formatResetTime(row.resetAt) : null;
            const barW = 110;
            const filled = Math.round((row.pct / 100) * barW);
            return (
              <div key={i} style={{ display: 'flex', flexDirection: 'column', gap: 1 }}>
                {/* 标签 + 百分比 + 重置 */}
                <div style={{
                  display: 'flex', alignItems: 'baseline', justifyContent: 'space-between',
                  fontFamily: 'monospace', fontWeight: 700,
                }}>
                  <span style={{ fontSize: 9, color: '#bbb' }}>{row.label}</span>
                  <span style={{ display: 'inline-flex', alignItems: 'baseline', gap: 3 }}>
                    {resetStr && (
                      <span style={{ fontSize: 8, color: '#666' }}>↻{resetStr}</span>
                    )}
                    <span style={{ fontSize: 10, color: pctColor }}>{row.pct.toFixed(0)}%</span>
                  </span>
                </div>
                {/* 进度条 */}
                <div style={{
                  width: barW, height: 3, borderRadius: 1.5,
                  background: 'rgba(255,255,255,0.12)', overflow: 'hidden',
                }}>
                  <div style={{
                    width: filled, height: '100%', borderRadius: 1.5,
                    background: row.color,
                    transition: 'width 0.5s ease',
                  }} />
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </>
  );
}

// ═══════════════════════════════════════════════════════════════
//  右侧滑出面板 — 设置（猫可见性）
// ═══════════════════════════════════════════════════════════════

function SettingsPanel({ showCc, showCx, showCatLabel, onToggleCc, onToggleCx, onToggleCatLabel, onClose }: {
  showCc: boolean;
  showCx: boolean;
  showCatLabel: boolean;
  onToggleCc: () => void;
  onToggleCx: () => void;
  onToggleCatLabel: () => void;
  onClose: () => void;
}) {
  const toggleTrack = (on: boolean): React.CSSProperties => ({
    width: 24, height: 12, borderRadius: 6,
    background: on ? '#a6e3a1' : '#444',
    position: 'relative', transition: 'background 0.2s',
    cursor: 'pointer', flexShrink: 0,
  });
  const toggleKnob = (on: boolean): React.CSSProperties => ({
    position: 'absolute', top: 1, left: on ? 13 : 1,
    width: 10, height: 10, borderRadius: 5,
    background: '#fff', transition: 'left 0.2s',
  });

  return (
    <>
      <div onClick={onClose} style={{
        position: 'absolute', inset: 0, zIndex: 10,
        background: 'rgba(0,0,0,0.3)',
      }} />
      <div style={{
        position: 'absolute', top: 0, right: 0, bottom: 0,
        width: 110, zIndex: 11,
        background: 'rgba(15,15,25,0.95)',
        borderLeft: '1px solid rgba(255,255,255,0.1)',
        backdropFilter: 'blur(8px)',
        display: 'flex', flexDirection: 'column',
        animation: 'slideInRight 0.2s ease-out',
        overflow: 'hidden',
      }}>
        <div style={{
          padding: '4px 8px 3px',
          borderBottom: '1px solid rgba(255,255,255,0.1)',
          display: 'flex', alignItems: 'center', justifyContent: 'space-between',
        }}>
          <span style={{ fontSize: 10, fontWeight: 900, fontFamily: 'monospace', color: '#aaa' }}>设置</span>
          <span onClick={onClose} style={{
            fontSize: 10, color: '#666', cursor: 'pointer', lineHeight: 1, padding: '0 2px',
          }}>✕</span>
        </div>
        <div style={{ padding: '6px 8px', display: 'flex', flexDirection: 'column', gap: 8 }}>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
            <span style={{ fontSize: 9, fontFamily: 'monospace', fontWeight: 700, color: BRAND_COLORS.cc }}>CC 猫</span>
            <div onClick={onToggleCc} style={toggleTrack(showCc)}>
              <div style={toggleKnob(showCc)} />
            </div>
          </div>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
            <span style={{ fontSize: 9, fontFamily: 'monospace', fontWeight: 700, color: BRAND_COLORS.cx }}>CX 猫</span>
            <div onClick={onToggleCx} style={toggleTrack(showCx)}>
              <div style={toggleKnob(showCx)} />
            </div>
          </div>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
            <span style={{ fontSize: 9, fontFamily: 'monospace', fontWeight: 700, color: '#aaa' }}>名字</span>
            <div onClick={onToggleCatLabel} style={toggleTrack(showCatLabel)}>
              <div style={toggleKnob(showCatLabel)} />
            </div>
          </div>
        </div>
      </div>
    </>
  );
}

// ═══════════════════════════════════════════════════════════════
//  精灵动画组件
// ═══════════════════════════════════════════════════════════════

function CatSprite({ anim, frame, colorFilter = '', flip = false, spriteUrl = catSpriteUrl }: {
  anim: AnimName;
  frame: number;
  colorFilter?: string;
  flip?: boolean;
  spriteUrl?: string;
}) {
  const def = ANIMS[anim];
  const col = frame % def.frames;

  // 计算裁切后的 background 偏移
  const sheetScale = SCALE * FRAME_SIZE * 8; // 整张精灵表缩放后宽度
  const bgX = -(col * FRAME_SIZE + CROP_X) * SCALE;
  const bgY = -(def.row * FRAME_SIZE + CROP_Y) * SCALE;

  return (
    <div style={{
      width: DISPLAY_W,
      height: DISPLAY_H,
      overflow: 'hidden',
      imageRendering: 'pixelated',
      transform: flip ? 'scaleX(-1)' : undefined,
      filter: colorFilter || undefined,
    }}>
      <div style={{
        width: DISPLAY_W,
        height: DISPLAY_H,
        backgroundImage: `url(${spriteUrl})`,
        backgroundSize: `${sheetScale}px auto`,
        backgroundPosition: `${bgX}px ${bgY}px`,
        backgroundRepeat: 'no-repeat',
      }} />
    </div>
  );
}

// ═══════════════════════════════════════════════════════════════
//  主组件
// ═══════════════════════════════════════════════════════════════

interface Agent {
  sessionId: string;
  source: 'claude' | 'codex';   // agent 来源
  session: Session | null;
  currentTool: ToolCall | null;
  hasError: boolean;             // 最近一次工具调用是否出错
  startTime: Date | null;        // 会话开始时间
  toolCount: number;             // 工具调用总次数
  title: string | null;          // agent 标题（第一条用户消息）
  slug: string | null;           // agent slug
  hasActiveTool?: boolean;       // Codex 用：是否有活跃工具
}

export function MiniMonitor() {
  const [agents, setAgents] = useState<Agent[]>([]);
  const [frame, setFrame] = useState(0);
  const [forceAnim, setForceAnim] = useState<AnimName | null>(null);
  const [timeOfDay, setTimeOfDay] = useState(getTimeOfDay);
  const [manualTheme, setManualTheme] = useState<'night' | 'dawn' | 'day' | 'dusk' | null>(null);
  const manualThemeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [activePanel, setActivePanel] = useState<'cc' | 'cx' | 'settings' | null>(null);
  const poll = useRef<ReturnType<typeof setInterval>>(null!);

  // ── 猫可见性设置（localStorage 持久化） ──
  const [showCc, setShowCc] = useState(() => localStorage.getItem('cat_show_cc') !== 'false');
  const [showCx, setShowCx] = useState(() => localStorage.getItem('cat_show_cx') !== 'false');
  const [showCatLabel, setShowCatLabel] = useState(() => localStorage.getItem('cat_show_label') === 'true');

  // 猫 hover 名字显示（名字关闭时，任一猫悬停则所有猫显示名字）
  const [catHovered, setCatHovered] = useState(false);

  const toggleShowCc = useCallback(() => {
    setShowCc(v => { localStorage.setItem('cat_show_cc', String(!v)); return !v; });
  }, []);
  const toggleShowCx = useCallback(() => {
    setShowCx(v => { localStorage.setItem('cat_show_cx', String(!v)); return !v; });
  }, []);
  const toggleShowCatLabel = useCallback(() => {
    setShowCatLabel(v => { localStorage.setItem('cat_show_label', String(!v)); return !v; });
  }, []);

  // 每分钟检查一次时间变化
  useEffect(() => {
    const id = setInterval(() => setTimeOfDay(getTimeOfDay()), 60_000);
    return () => clearInterval(id);
  }, []);

  const theme = THEMES[manualTheme ?? timeOfDay];

  // 快捷键 Space：循环切换背景主题，1 分钟后恢复按时间自动显示
  const THEME_CYCLE = ['night', 'dawn', 'day', 'dusk'] as const;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.code === 'Space') {
        e.preventDefault();
        setManualTheme(prev => {
          const current = prev ?? timeOfDay;
          const idx = THEME_CYCLE.indexOf(current);
          return THEME_CYCLE[(idx + 1) % THEME_CYCLE.length];
        });
        if (manualThemeTimer.current) clearTimeout(manualThemeTimer.current);
        manualThemeTimer.current = setTimeout(() => setManualTheme(null), 60_000);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [timeOfDay]);

  // cc / cx 各自独立判断是否活跃
  const ccActive = agents.some(a => a.source === 'claude');
  const cxActive = agents.some(a => a.source === 'codex');
  const prevCcActive = useRef(false);
  const prevCxActive = useRef(false);

  // ─── 每只猫的位置/朝向（固定 key: _cc / _cx，不随 agent 增减变化）───
  interface CatPos { x: number; dir: 1 | -1; }
  const catPositions = useRef<Map<string, CatPos>>(new Map());
  const CC_KEY = '_cc';
  const CX_KEY = '_cx';

  // 加权池：sit/play 权重高，run/pounce 权重低（偶尔出现）
  const IDLE_WEIGHTED: { anim: AnimName; weight: number }[] = [
    { anim: 'sit1', weight: 5 }, { anim: 'sit2', weight: 5 },
    { anim: 'sit3', weight: 5 }, { anim: 'sit4', weight: 5 },
    { anim: 'play', weight: 5 },
    { anim: 'run1', weight: 3 }, { anim: 'run2', weight: 3 },
    { anim: 'pounce', weight: 3 },
  ];
  const IDLE_TOTAL_WEIGHT = IDLE_WEIGHTED.reduce((s, e) => s + e.weight, 0);
  const pickIdleAnim = (): AnimName => {
    let r = Math.random() * IDLE_TOTAL_WEIGHT;
    for (const e of IDLE_WEIGHTED) {
      r -= e.weight;
      if (r <= 0) return e.anim;
    }
    return 'sit1';
  };

  // 每只猫的动画 — 固定 key: _cc / _cx
  const perCatIdle = useRef<Map<string, AnimName>>(new Map());
  const [idleTick, setIdleTick] = useState(0);
  const ccStretchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cxStretchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // cc 猫独立的 idle <-> active 过渡
  useEffect(() => {
    const justActivated = !prevCcActive.current && ccActive;
    prevCcActive.current = ccActive;

    if (!ccActive || forceAnim) {
      perCatIdle.current.set(CC_KEY, 'sleep');
      if (ccStretchTimer.current) { clearTimeout(ccStretchTimer.current); ccStretchTimer.current = null; }
      setIdleTick(t => t + 1);
      return;
    }
    if (justActivated) {
      perCatIdle.current.set(CC_KEY, 'stretch');
      setIdleTick(t => t + 1);
      ccStretchTimer.current = setTimeout(() => {
        ccStretchTimer.current = null;
        perCatIdle.current.set(CC_KEY, pickIdleAnim());
        setIdleTick(t => t + 1);
      }, 2400);
    }
  }, [ccActive, forceAnim]);

  // cx 猫独立的 idle <-> active 过渡
  useEffect(() => {
    const justActivated = !prevCxActive.current && cxActive;
    prevCxActive.current = cxActive;

    if (!cxActive || forceAnim) {
      perCatIdle.current.set(CX_KEY, 'sleep');
      if (cxStretchTimer.current) { clearTimeout(cxStretchTimer.current); cxStretchTimer.current = null; }
      setIdleTick(t => t + 1);
      return;
    }
    if (justActivated) {
      perCatIdle.current.set(CX_KEY, 'stretch');
      setIdleTick(t => t + 1);
      cxStretchTimer.current = setTimeout(() => {
        cxStretchTimer.current = null;
        perCatIdle.current.set(CX_KEY, pickIdleAnim());
        if (showCc) checkOverlapAndFlee(CX_KEY);
        setIdleTick(t => t + 1);
      }, 2400);
    }
  }, [cxActive, forceAnim]);

  // 检测两猫是否相交，是则让 CX 猫跑开
  const checkOverlapAndFlee = useCallback((catKey: string) => {
    const otherKey = catKey === CX_KEY ? CC_KEY : CX_KEY;
    const myPos = catPositions.current.get(catKey);
    const otherPos = catPositions.current.get(otherKey);
    if (!myPos || !otherPos) return false;
    const dist = Math.abs(myPos.x - otherPos.x);
    if (dist < OVERLAP_THRESHOLD) {
      // 相交：切换为跑步动画，方向远离对方
      const runAnim = RUN_ANIMS[Math.floor(Math.random() * RUN_ANIMS.length)];
      perCatIdle.current.set(catKey, runAnim);
      myPos.dir = myPos.x >= otherPos.x ? 1 : -1;
      return true;
    }
    return false;
  }, []);

  // 两猫都可见时才需要碰撞检测
  const bothVisible = showCc && showCx;

  // 每 10 秒：只有活跃的猫才轮换动画
  useEffect(() => {
    if (forceAnim) return;
    if (!ccActive && !cxActive) return;
    const id = setInterval(() => {
      if (ccActive) perCatIdle.current.set(CC_KEY, pickIdleAnim());
      if (cxActive) perCatIdle.current.set(CX_KEY, pickIdleAnim());
      // CX 活跃且两猫都可见时，检测是否重叠
      if (cxActive && bothVisible) checkOverlapAndFlee(CX_KEY);
      setIdleTick(t => t + 1);
    }, 10_000);
    return () => clearInterval(id);
  }, [ccActive, cxActive, forceAnim, checkOverlapAndFlee, bothVisible]);

  // 清理 stretch 定时器
  useEffect(() => {
    return () => {
      if (ccStretchTimer.current) clearTimeout(ccStretchTimer.current);
      if (cxStretchTimer.current) clearTimeout(cxStretchTimer.current);
    };
  }, []);

  const refresh = useCallback(async () => {
    try {
      // ── Claude Code agents ──
      const list = await invoke<ActiveSessionInfo[]>('get_active_sessions');
      const claudeAgents: Agent[] = [];
      for (const s of list) {
        let ses: Session | null = null;
        try {
          ses = await invoke<Session | null>('load_session_silent', { filePath: s.filePath, subAgentFiles: s.subAgentFiles });
        } catch { /* load_session_silent 失败不影响 agent 列表 */ }
        const tools: ToolCall[] = [];
        if (ses) {
          for (const t of ses.turns) for (const tc of t.toolCalls) tools.push(tc);
          if (ses.subAgents) for (const r of ses.subAgents) if (r.session)
            for (const t of r.session.turns) for (const tc of t.toolCalls) tools.push(tc);
        }
        const last = tools.length ? tools[tools.length - 1] : null;
        const lastDone = [...tools].reverse().find(tc => tc.endTime !== null);
        const hasError = lastDone?.isError ?? false;
        const startTime = tools.length ? tools[0].startTime : null;
        claudeAgents.push({
          sessionId: s.sessionId, source: 'claude', session: ses,
          currentTool: last?.endTime === null ? last : null,
          hasError, startTime, toolCount: tools.length,
          title: s.title ?? null, slug: s.slug ?? null,
        });
      }

      // ── Codex agents ──
      let codexAgents: Agent[] = [];
      try {
        const codexList = await invoke<ActiveCodexSessionInfo[]>('get_active_codex_sessions');
        codexAgents = codexList.map(s => ({
          sessionId: s.sessionId, source: 'codex' as const,
          session: null, currentTool: null,
          hasActiveTool: s.hasActiveTool, hasError: s.hasError,
          startTime: s.startTime ? new Date(s.startTime) : null,
          toolCount: s.toolCount, title: s.title, slug: null,
        }));
      } catch { /* codex commands may not be implemented yet */ }

      setAgents([...claudeAgents, ...codexAgents]);
    } catch { /* noop */ }
  }, []);

  useEffect(() => {
    refresh();
    poll.current = setInterval(refresh, 3000);
    return () => clearInterval(poll.current);
  }, [refresh]);

  // 动画帧 — 250ms 一帧，舒缓的精灵动画
  useEffect(() => {
    const id = setInterval(() => setFrame(f => f + 1), 250);
    return () => clearInterval(id);
  }, []);

  // 每秒更新时间（用于运行时长显示）
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  // ── Claude 用量数据（订阅用户用） ──
  // 后端有 5 分钟缓存，进入 mini 模式时读缓存（不会真正调 API）
  // cost 停止变化 5 分钟后再刷新一次（仅一次）
  const [userInfo, setUserInfo] = useState<ClaudeUserInfo | null>(null);
  const lastCostRef = useRef<number>(0);
  const stableTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const hasFetchedAfterStable = useRef(false);
  const COST_STABLE_DELAY = 300_000; // 5 分钟
  const USAGE_THROTTLE = 600_000;    // 渲染层节流：10 分钟内不重复请求

  // ── cc 额度（带节流 + 延迟队列） ──
  const lastCcFetchAt = useRef(0);
  const deferredCcTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const fetchUserInfo = useCallback(async () => {
    const now = Date.now();
    const elapsed = now - lastCcFetchAt.current;
    if (elapsed < USAGE_THROTTLE) {
      // 节流中 → 延迟到窗口过期后再刷新（合并多次请求）
      if (!deferredCcTimer.current) {
        const remaining = USAGE_THROTTLE - elapsed;
        deferredCcTimer.current = setTimeout(() => {
          deferredCcTimer.current = null;
          fetchUserInfo();
        }, remaining);
      }
      return;
    }
    lastCcFetchAt.current = now;
    try {
      const info = await invoke<ClaudeUserInfo>('get_claude_user_info');
      setUserInfo(info);
    } catch (e) {
      console.warn('[MiniMonitor] fetchUserInfo failed:', e);
    }
  }, []);

  // 进入迷你模式时立即获取一次
  useEffect(() => { fetchUserInfo(); }, [fetchUserInfo]);

  const isSubscriber = userInfo?.userType === 'subscriber';
  const hasUsage = isSubscriber && userInfo?.usage != null;

  // ── cx 额度（带节流 + 延迟队列） ──
  const [codexUsage, setCodexUsage] = useState<CodexRateLimits | null>(null);
  const lastCxFetchAt = useRef(0);
  const deferredCxTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const fetchCodexUsage = useCallback(async () => {
    const now = Date.now();
    const elapsed = now - lastCxFetchAt.current;
    if (elapsed < USAGE_THROTTLE) {
      // 节流中 → 延迟到窗口过期后再刷新（合并多次请求）
      if (!deferredCxTimer.current) {
        const remaining = USAGE_THROTTLE - elapsed;
        deferredCxTimer.current = setTimeout(() => {
          deferredCxTimer.current = null;
          fetchCodexUsage();
        }, remaining);
      }
      return;
    }
    lastCxFetchAt.current = now;
    try {
      const usage = await invoke<CodexRateLimits | null>('get_codex_usage');
      setCodexUsage(usage);
    } catch (e) {
      console.warn('[MiniMonitor] fetchCodexUsage failed:', e);
    }
  }, []);

  // 进入迷你模式时获取一次
  useEffect(() => { fetchCodexUsage(); }, [fetchCodexUsage]);

  // 组件卸载时清理延迟定时器
  useEffect(() => {
    return () => {
      if (deferredCcTimer.current) clearTimeout(deferredCcTimer.current);
      if (deferredCxTimer.current) clearTimeout(deferredCxTimer.current);
    };
  }, []);

  // ── 到达 resets_at 时自动刷新 ──
  useEffect(() => {
    const timers: ReturnType<typeof setTimeout>[] = [];
    // cc: 取所有 resets_at
    if (userInfo?.usage) {
      const ccResets = [
        userInfo.usage.five_hour.resets_at,
        userInfo.usage.seven_day.resets_at,
        userInfo.usage.seven_day_sonnet?.resets_at,
      ].filter(Boolean) as string[];
      for (const r of ccResets) {
        const delay = new Date(r).getTime() - Date.now() + 2000; // 多等 2 秒确保后端已重置
        if (delay > 0 && delay < 86400_000) {
          timers.push(setTimeout(() => fetchUserInfo(), delay));
        }
      }
    }
    // cx: 取 primary / secondary 的 resets_at
    if (codexUsage) {
      const cxResets = [codexUsage.primary.resets_at, codexUsage.secondary.resets_at];
      for (const r of cxResets) {
        const delay = r * 1000 - Date.now() + 2000;
        if (delay > 0 && delay < 86400_000) {
          timers.push(setTimeout(() => fetchCodexUsage(), delay));
        }
      }
    }
    return () => timers.forEach(clearTimeout);
  }, [userInfo?.usage, codexUsage, fetchUserInfo, fetchCodexUsage]);

  const cost = agents.reduce((s, a) => s + (a.session?.tokenUsage.estimatedCost ?? 0), 0);

  // cost 变化时重置 5 分钟定时器；稳定 5 分钟后 cc/cx 额度各刷新一次
  useEffect(() => {
    if (cost !== lastCostRef.current) {
      lastCostRef.current = cost;
      hasFetchedAfterStable.current = false;
      if (stableTimerRef.current) clearTimeout(stableTimerRef.current);
      stableTimerRef.current = setTimeout(() => {
        if (!hasFetchedAfterStable.current) {
          hasFetchedAfterStable.current = true;
          fetchUserInfo();
          fetchCodexUsage();
        }
      }, COST_STABLE_DELAY);
    }
    return () => {
      if (stableTimerRef.current) clearTimeout(stableTimerRef.current);
    };
  }, [cost, fetchUserInfo, fetchCodexUsage]);

  // 获取或初始化猫的位置（新猫放在离已有猫最远的空隙处）
  const getCatPos = useCallback((key: string): CatPos => {
    if (!catPositions.current.has(key)) {
      const minX = CAT_MARGIN;
      const maxX = WINDOW_W - DISPLAY_SIZE - CAT_MARGIN;
      const existing = [...catPositions.current.values()].map(p => p.x).sort((a, b) => a - b);
      let x: number;
      if (existing.length === 0) {
        x = (WINDOW_W - DISPLAY_SIZE) / 2;
      } else {
        let bestGap = 0, bestX = minX;
        const gapLeft = existing[0] - minX;
        if (gapLeft > bestGap) { bestGap = gapLeft; bestX = minX; }
        for (let i = 1; i < existing.length; i++) {
          const gap = existing[i] - existing[i - 1];
          if (gap > bestGap) { bestGap = gap; bestX = existing[i - 1] + gap / 2; }
        }
        const gapRight = maxX - existing[existing.length - 1];
        if (gapRight > bestGap) { bestX = maxX; }
        x = Math.max(minX, Math.min(maxX, bestX));
      }
      catPositions.current.set(key, { x, dir: Math.random() > 0.5 ? 1 : -1 as (1 | -1) });
    }
    return catPositions.current.get(key)!;
  }, []);

  // 计算猫的动画：直接从 perCatIdle 取（由 cc/cx 独立的 effect 维护）
  const resolveCatAnim = useCallback((catKey: string): AnimName => {
    if (forceAnim) return forceAnim;
    return perCatIdle.current.get(catKey) ?? 'sleep';
  }, [forceAnim, idleTick]);

  // 缓存每只猫当前的解析动画，供帧更新使用
  const resolvedAnims = useRef<Map<string, AnimName>>(new Map());

  // 每帧更新所有移动中的猫的位置（与帧率同步）
  useEffect(() => {
    for (const [key, pos] of catPositions.current.entries()) {
      const anim = resolvedAnims.current.get(key) ?? 'sleep';
      if (!MOVING_ANIMS.has(anim)) continue;
      const px = MOVE_PX[anim] ?? 2;
      const minX = CAT_MARGIN;
      const maxX = WINDOW_W - DISPLAY_SIZE - CAT_MARGIN;
      let next = pos.x + px * pos.dir;
      if (next >= maxX) { next = maxX; pos.dir = -1; }
      if (next <= minX) { next = minX; pos.dir = 1; }
      pos.x = next;
    }
  }, [frame]);

  return (
    <div
      data-tauri-drag-region
      style={{
        position: 'relative',
        height: '100vh', display: 'flex', flexDirection: 'column',
        background: theme.sky, overflow: 'hidden', userSelect: 'none',
        borderRadius: 10,
      }}>
      {/* 标题栏 */}
      <div data-tauri-drag-region style={{
        display: 'flex', alignItems: 'center', justifyContent: 'space-between',
        padding: '3px 6px', background: theme.titleBar,
        borderBottom: `1px solid ${theme.border}`, flexShrink: 0,
      }}>
        <span
          onClick={() => getCurrentWindow().hide()}
          style={{
            width: 8, height: 8, borderRadius: '50%',
            background: '#ff5f57',
            cursor: 'pointer',
            flexShrink: 0,
            transition: 'opacity 0.15s',
          }}
          onMouseEnter={(e) => { (e.target as HTMLElement).style.opacity = '0.7'; }}
          onMouseLeave={(e) => { (e.target as HTMLElement).style.opacity = '1'; }}
          title="退出"
        />
        <span
          onClick={() => setActivePanel(p => p === 'settings' ? null : 'settings')}
          style={{
            fontSize: 11, color: '#999', cursor: 'pointer',
            lineHeight: 1, padding: '0 2px',
            transition: 'color 0.15s',
          }}
          onMouseEnter={(e) => { (e.target as HTMLElement).style.color = '#fff'; }}
          onMouseLeave={(e) => { (e.target as HTMLElement).style.color = '#999'; }}
          title="设置"
        >⚙</span>
      </div>

      {/* 猫游乐场 */}
      <div style={{
        flex: 1, display: 'flex', flexDirection: 'column',
        alignItems: 'center', justifyContent: 'flex-end',
        position: 'relative', overflow: 'hidden',
      }}>
        {/* 星星 — 夜晚/黄昏/黎明可见 */}
        {theme.showStars && (
          <div style={{
            position: 'absolute', inset: 0, pointerEvents: 'none',
          }}>
            {[
              { x: 12, y: 8 }, { x: 45, y: 18 }, { x: 88, y: 5 },
              { x: 128, y: 22 }, { x: 160, y: 10 }, { x: 70, y: 24 },
              { x: 30, y: 30 }, { x: 148, y: 34 }, { x: 105, y: 12 },
            ].map((s, i) => (
              <span key={i} style={{
                position: 'absolute', left: s.x, top: s.y,
                width: 2, height: 2, background: theme.starColor, borderRadius: 1,
                animation: `twinkle 3s ${i * 0.4}s infinite`,
              }} />
            ))}
          </div>
        )}

        {/* 白天云朵 */}
        {timeOfDay === 'day' && (
          <div style={{ position: 'absolute', inset: 0, pointerEvents: 'none', overflow: 'hidden' }}>
            {[
              { y: 6, w: 28, h: 8, dur: 50, delay: 0 },
              { y: 14, w: 20, h: 6, dur: 60, delay: 20 },
            ].map((c, i) => (
              <div key={i} style={{
                position: 'absolute', top: c.y, left: -(c.w + 20),
                width: c.w, height: c.h,
                background: 'rgba(255,255,255,0.6)',
                borderRadius: c.h,
                boxShadow: `${c.w * 0.3}px ${c.h * -0.2}px 0 rgba(255,255,255,0.4), ${c.w * 0.55}px ${c.h * 0.1}px 0 rgba(255,255,255,0.3)`,
                animation: `cloudDrift ${c.dur}s ${c.delay}s linear infinite`,
              }} />
            ))}
          </div>
        )}

        {/* 白天太阳 / 夜晚月亮 */}
        {timeOfDay === 'day' ? (
          <div style={{
            position: 'absolute', top: 6, right: 14,
            width: 16, height: 16, borderRadius: '50%',
            background: '#ffdd44',
            boxShadow: '0 0 8px rgba(255,221,68,0.5), 0 0 20px rgba(255,221,68,0.2)',
          }} />
        ) : timeOfDay === 'night' ? (
          <div style={{
            position: 'absolute', top: 8, right: 14,
            width: 14, height: 14, borderRadius: '50%',
            background: '#ffdd88',
            boxShadow: '0 0 6px rgba(255,221,136,0.3), inset -4px -1px 0 0 #12121e',
          }} />
        ) : timeOfDay === 'dawn' ? (
          <div style={{
            position: 'absolute', top: 10, right: 14,
            width: 14, height: 14, borderRadius: '50%',
            background: '#ffaa55',
            boxShadow: '0 0 10px rgba(255,170,85,0.4)',
          }} />
        ) : /* dusk */ (
          <div style={{
            position: 'absolute', top: 8, right: 14,
            width: 14, height: 14, borderRadius: '50%',
            background: '#ff8844',
            boxShadow: '0 0 10px rgba(255,136,68,0.4)',
          }} />
        )}

        {/* 时钟 — 两只猫都隐藏时显示 */}
        {!showCc && !showCx && (
          <div style={{
            position: 'absolute', inset: 0,
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            pointerEvents: 'none',
          }}>
            <span style={{
              fontSize: 30, fontFamily: 'monospace', fontWeight: 700,
              color: 'rgba(255,255,255,0.12)',
              letterSpacing: 2,
            }}>
              {new Date(now).toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit', hour12: false })}
            </span>
          </div>
        )}

        {/* 猫 — 按可见性设置显示 */}
        {(() => {
          // 构建渲染列表
          type CatEntry = {
            key: string; agent: Agent | null; anim: AnimName; colorFilter: string;
            spriteUrl: string;
          };
          const cats: CatEntry[] = [];

          const claudeAgents = agents.filter(a => a.source === 'claude');
          const codexAgents = agents.filter(a => a.source === 'codex');

          // 每种类型始终只有一只猫，取最活跃的 agent 代表该类型
          const pickRepresentative = (list: Agent[]): Agent | null => {
            if (list.length === 0) return null;
            // 优先级：有错误 > 正在执行工具 > 第一个
            return list[0];
          };

          // Claude 猫（cc）
          if (showCc) {
            const anim = resolveCatAnim(CC_KEY);
            resolvedAnims.current.set(CC_KEY, anim);
            cats.push({ key: CC_KEY, agent: pickRepresentative(claudeAgents), anim, colorFilter: '', spriteUrl: catSpriteUrl });
          }

          // Codex 猫（cx）
          if (showCx) {
            const anim = resolveCatAnim(CX_KEY);
            resolvedAnims.current.set(CX_KEY, anim);
            cats.push({ key: CX_KEY, agent: pickRepresentative(codexAgents), anim, colorFilter: '', spriteUrl: cat2SpriteUrl });
          }

          return cats.map((cat) => {
            const pos = getCatPos(cat.key);
            const showZzz = cat.anim === 'sleep';

            const labelText = cat.key === CX_KEY ? 'cx' : 'cc';
            const showLabel = !forceAnim && (showCatLabel || catHovered);

            return (
              <div key={cat.key} style={{
                position: 'absolute', bottom: -5, left: pos.x,
                zIndex: 1,
              }}
                onMouseEnter={() => setCatHovered(true)}
                onMouseLeave={() => setCatHovered(false)}
              >
                <div style={{ position: 'relative', display: 'inline-block' }}>
                  {/* 猫头顶标签：常显 or hover 浮现 */}
                  {showLabel && (
                    <div style={{
                      position: 'absolute',
                      left: '50%', transform: 'translateX(-50%)',
                      bottom: 30,
                      zIndex: 2,
                      fontSize: 7, fontWeight: 900, fontFamily: 'monospace',
                      color: '#fff',
                      whiteSpace: 'nowrap',
                      textShadow: '0 0 4px rgba(0,0,0,0.9), 0 1px 2px rgba(0,0,0,0.7)',
                      lineHeight: 1,
                    }}>{labelText}</div>
                  )}
                  <CatSprite
                    anim={cat.anim}
                    frame={frame}
                    colorFilter={cat.colorFilter}
                    flip={pos.dir === -1}
                    spriteUrl={cat.spriteUrl}
                  />
                  {/* zzz — 仅睡觉动画时显示 */}
                  {showZzz && [0, 1, 2].map(i => (
                    <span key={i} style={{
                      position: 'absolute',
                      left: DISPLAY_SIZE / 2 + 6 + i * 8,
                      bottom: 16 + i * 6,
                      fontSize: 7 + i * 2, fontFamily: 'monospace', fontWeight: 900,
                      color: '#4466aa', animation: `floatZ 2.5s ${i * 0.5}s infinite`,
                    }}>z</span>
                  ))}
                </div>
              </div>
            );
          });
        })()}
      </div>

      {/* 地面 */}
      <div style={{
        height: 14, flexShrink: 0, position: 'relative',
        overflow: 'hidden', marginTop: -7,
      }}>
        {/* 地面色带 */}
        <div style={{
          position: 'absolute', bottom: 0, left: 0, right: 0, height: 8,
          background: `linear-gradient(to bottom, ${theme.groundTop}, ${theme.groundBot})`,
        }} />
        {/* 草 */}
        <div style={{
          position: 'absolute', bottom: 0, left: 0,
          width: 180, height: '100%',
        }}>
          {[6, 20, 36, 54, 72, 92, 110, 128, 146, 164].map((x, i) => (
            <span key={i} style={{
              position: 'absolute', bottom: 6, left: x,
              width: 2, height: i % 3 === 0 ? 6 : i % 2 === 0 ? 5 : 4,
              background: i % 3 === 0 ? theme.grassA : theme.grassB,
            }} />
          ))}
        </div>
      </div>

      {/* 底部状态栏：cc/cx 额度水平排列，点击打开详情面板 */}
      <div style={{
        padding: '2px 4px', display: 'flex', flexWrap: 'wrap',
        justifyContent: 'center', alignItems: 'center', gap: '1px 6px',
        flexShrink: 0,
        background: theme.statusBar, minHeight: 14,
      }}>
        {hasUsage && (
          <span
            onClick={(e) => { e.stopPropagation(); setActivePanel(p => p === 'cc' ? null : 'cc'); }}
            style={{ cursor: 'pointer', display: 'inline-flex', alignItems: 'baseline', gap: 4 }}
          >
            <UsageTag label="cc·5h" pct={userInfo!.usage!.five_hour.utilization} />
          </span>
        )}
        {hasUsage && codexUsage && (
          <span style={{ width: 6, display: 'inline-block' }} />
        )}
        {codexUsage && (
          <span
            onClick={(e) => { e.stopPropagation(); setActivePanel(p => p === 'cx' ? null : 'cx'); }}
            style={{ cursor: 'pointer', display: 'inline-flex', alignItems: 'baseline', gap: 4 }}
          >
            <UsageTag label="cx·5h" pct={codexUsage.primary.used_percent} />
          </span>
        )}
        {!hasUsage && !codexUsage && (
          <span style={{ fontSize: 7, fontFamily: 'monospace', color: '#555' }}>—</span>
        )}
      </div>

      {/* 右侧滑出详情面板 */}
      {activePanel === 'cc' && hasUsage && (
        <UsageDetailPanel
          type="cc"
          onClose={() => setActivePanel(null)}
          rows={[
            {
              label: '5 小时窗口',
              pct: userInfo!.usage!.five_hour.utilization,
              color: userInfo!.usage!.five_hour.utilization > 80 ? '#ff6b6b' : '#a6e3a1',
              resetAt: userInfo!.usage!.five_hour.resets_at,
            },
            {
              label: '7 天总量',
              pct: userInfo!.usage!.seven_day.utilization,
              color: userInfo!.usage!.seven_day.utilization > 80 ? '#ff6b6b' : '#88aaee',
              resetAt: userInfo!.usage!.seven_day.resets_at,
            },
            ...(userInfo!.usage!.seven_day_sonnet ? [{
              label: '7 天 Sonnet',
              pct: userInfo!.usage!.seven_day_sonnet!.utilization,
              color: userInfo!.usage!.seven_day_sonnet!.utilization > 80 ? '#ff6b6b' : '#c4a6e8',
              resetAt: userInfo!.usage!.seven_day_sonnet!.resets_at,
            }] : []),
          ]}
        />
      )}
      {activePanel === 'cx' && codexUsage && (
        <UsageDetailPanel
          type="cx"
          onClose={() => setActivePanel(null)}
          rows={[
            {
              label: '5 小时窗口',
              pct: codexUsage.primary.used_percent,
              color: codexUsage.primary.used_percent > 80 ? '#ff6b6b' : '#a6e3a1',
              resetAt: new Date(codexUsage.primary.resets_at * 1000).toISOString(),
            },
            {
              label: '7 天总量',
              pct: codexUsage.secondary.used_percent,
              color: codexUsage.secondary.used_percent > 80 ? '#ff6b6b' : '#88aaee',
              resetAt: new Date(codexUsage.secondary.resets_at * 1000).toISOString(),
            },
          ]}
        />
      )}

      {/* 设置面板 */}
      {activePanel === 'settings' && (
        <SettingsPanel
          showCc={showCc}
          showCx={showCx}
          showCatLabel={showCatLabel}
          onToggleCc={toggleShowCc}
          onToggleCx={toggleShowCx}
          onToggleCatLabel={toggleShowCatLabel}
          onClose={() => setActivePanel(null)}
        />
      )}

      <style>{`
        @keyframes floatZ {
          0% { opacity: 0.9; transform: translateY(0) scale(1); }
          60% { opacity: 0.5; }
          100% { opacity: 0; transform: translateY(-14px) scale(1.15); }
        }
        @keyframes twinkle {
          0%, 100% { opacity: 0.15; }
          50% { opacity: 0.75; }
        }
        @keyframes cloudDrift {
          0% { transform: translateX(0); }
          100% { transform: translateX(260px); }
        }
        @keyframes alertBlink {
          0%, 100% { opacity: 1; }
          50% { opacity: 0.5; }
        }
        @keyframes alertBounce {
          0%, 100% { transform: translateY(0) scale(1); }
          50% { transform: translateY(-3px) scale(1.1); }
        }
        @keyframes slideInRight {
          from { transform: translateX(100%); opacity: 0; }
          to   { transform: translateX(0);    opacity: 1; }
        }
      `}</style>
    </div>
  );
}
