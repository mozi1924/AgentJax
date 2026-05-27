import { useEffect, useRef, useState } from 'react';
import type { RefObject } from 'react';
import { ChevronDown, ChevronRight, Menu } from 'lucide-react';
import {
  DEFAULT_REASONING_MODE,
} from '../features/models/modelCatalog';
import type { ModelOption } from '../features/conversations/types';

const formatReasoningLabel = (value: string) => {
  const normalized = `${value || ''}`.trim().toLowerCase();
  if (!normalized || normalized === DEFAULT_REASONING_MODE) {
    return '跟随配置';
  }
  if (normalized === 'none') {
    return 'None';
  }
  return normalized.toUpperCase();
};

const formatProviderModelLabel = (option: ModelOption) => {
  const provider = `${option?.providerKey || ''}`.trim();
  const profile = `${option?.profileKey || ''}`.trim();
  if (!provider) {
    return profile || 'provider';
  }
  if (profile.startsWith(`${provider}/`)) {
    return `${provider} / ${profile.slice(provider.length + 1)}`;
  }
  return `${provider} / ${profile}`;
};

interface AppHeaderProps {
  titlebarRef: RefObject<HTMLDivElement | null>;
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  selectedModel: string;
  selectedModelOption: ModelOption | null;
  modelOptions: ModelOption[];
  onSelectModel: (profileKey: string) => void;
  selectedReasoningMode: string;
  onSelectReasoningMode: (value: string) => void;
  configPath: string;
  cachePath: string;
}

export default function AppHeader({
  titlebarRef,
  sidebarOpen,
  onToggleSidebar,
  selectedModel,
  selectedModelOption,
  modelOptions,
  onSelectModel,
  selectedReasoningMode,
  onSelectReasoningMode,
  configPath,
  cachePath,
}: AppHeaderProps) {
  const [modelDropdownOpen, setModelDropdownOpen] = useState(false);
  const [activeSubmenuKey, setActiveSubmenuKey] = useState<string | null>(null);
  const dropdownRef = useRef<HTMLDivElement | null>(null);
  const submenuTimeoutRef = useRef<number | null>(null);

  const handleMouseEnterRow = (key: string | null) => {
    if (submenuTimeoutRef.current) {
      window.clearTimeout(submenuTimeoutRef.current);
      submenuTimeoutRef.current = null;
    }
    setActiveSubmenuKey(key);
  };

  const handleMouseLeaveDropdown = () => {
    submenuTimeoutRef.current = window.setTimeout(() => {
      setActiveSubmenuKey(null);
    }, 150);
  };

  const handleMouseEnterSubmenu = () => {
    if (submenuTimeoutRef.current) {
      window.clearTimeout(submenuTimeoutRef.current);
      submenuTimeoutRef.current = null;
    }
  };

  useEffect(() => {
    return () => {
      if (submenuTimeoutRef.current) {
        window.clearTimeout(submenuTimeoutRef.current);
      }
    };
  }, []);

  useEffect(() => {
    const handlePointerDown = (event: PointerEvent) => {
      if (!dropdownRef.current?.contains(event.target as Node)) {
        setModelDropdownOpen(false);
      }
    };

    document.addEventListener('pointerdown', handlePointerDown);
    return () => {
      document.removeEventListener('pointerdown', handlePointerDown);
    };
  }, []);

  const hasReasoningSupport = !!selectedModelOption?.supportsReasoning;
  const reasoningOptions =
    hasReasoningSupport && Array.isArray(selectedModelOption?.supportedReasoningLevels)
      ? selectedModelOption.supportedReasoningLevels
      : [];

  const selectedReasoningLabel = hasReasoningSupport
    ? formatReasoningLabel(selectedReasoningMode)
    : '';

  return (
    <div
      className="absolute inset-x-0 top-0 z-40 flex h-12 items-center border-b border-[#2d2f31]/40 bg-[#131314]/90 pl-[84px] backdrop-blur"
      ref={titlebarRef}
    >
      <button
        onClick={onToggleSidebar}
        data-no-drag="true"
        className="flex h-7 w-7 flex-shrink-0 items-center justify-center rounded-full text-slate-300 transition hover:bg-[#2d2f31]"
        title={sidebarOpen ? '收起菜单' : '展开菜单'}
      >
        <Menu className="h-4.5 w-4.5" />
      </button>

      <div className="ml-2 flex min-w-0 flex-1 items-center gap-1 pr-6">
        <div ref={dropdownRef} className="relative flex items-center gap-1.5" data-no-drag="true">
          <button
            onClick={() => setModelDropdownOpen((current) => !current)}
            className="flex items-center gap-2 whitespace-nowrap rounded-xl px-3 py-1 text-sm font-medium text-slate-300 transition hover:bg-[#2d2f31]"
            title={
              configPath
                ? `配置文件: ${configPath}${cachePath ? `\n模型缓存: ${cachePath}` : ''}`
                : '模型配置'
            }
          >
            <span className="truncate">
              AgentJax {selectedModelOption?.modelId || selectedModel}
            </span>
            {selectedReasoningLabel && (
              <span className="rounded-full border border-indigo-500/20 bg-indigo-500/10 px-2 py-0.5 text-[11px] text-indigo-200">
                {selectedReasoningLabel}
              </span>
            )}
            <ChevronDown className="h-3 w-3 text-slate-400" />
          </button>

          {modelDropdownOpen && (
            <div
              onMouseLeave={handleMouseLeaveDropdown}
              className="absolute left-0 top-10 z-50 w-[320px] rounded-2xl border border-[#2d2f31] bg-[#1e1f20] p-2 shadow-2xl"
            >
              <div
                onMouseEnter={() => {
                  if (hasReasoningSupport) {
                    handleMouseEnterRow('reasoning');
                  }
                }}
                className={`relative flex w-full items-center justify-between gap-3 rounded-2xl px-3 py-2.5 text-left transition select-none ${
                  hasReasoningSupport
                    ? 'group cursor-pointer text-slate-200 hover:bg-[#2d2f31]'
                    : 'cursor-not-allowed text-slate-400 opacity-45'
                }`}
              >
                <div className="min-w-0 flex-1">
                  <span className="block text-sm font-medium">推理等级 (Reasoning)</span>
                  <span className="mt-0.5 block truncate text-[10px] text-slate-500">
                    {hasReasoningSupport
                      ? '选择全局思考努力程度'
                      : '当前选中模型不支持思考等级'}
                  </span>
                </div>
                {hasReasoningSupport && (
                  <div className="flex shrink-0 select-none items-center gap-1.5 text-xs font-medium text-indigo-400">
                    <span>{formatReasoningLabel(selectedReasoningMode)}</span>
                    <ChevronRight className="h-3.5 w-3.5 text-slate-500 transition group-hover:text-slate-300" />
                  </div>
                )}
              </div>

              {activeSubmenuKey === 'reasoning' && hasReasoningSupport && (
                <div
                  onMouseEnter={handleMouseEnterSubmenu}
                  className="sidebar-context-menu before:content-[''] absolute left-[324px] top-0 z-50 w-[180px] rounded-2xl border border-[#2d2f31] bg-[#1e1f20] p-2 shadow-2xl before:absolute before:-left-3 before:top-0 before:bottom-0 before:w-3"
                >
                  <div className="mb-1 border-b border-[#2d2f31]/60 px-3 py-1.5 text-[11px] font-semibold tracking-[0.2em] text-slate-500 uppercase">
                    选择推理等级
                  </div>
                  <div className="grid gap-0.5">
                    <button
                      onClick={(event) => {
                        event.stopPropagation();
                        onSelectReasoningMode(DEFAULT_REASONING_MODE);
                        setModelDropdownOpen(false);
                      }}
                      className={`flex cursor-pointer items-center justify-between rounded-xl px-2.5 py-1.5 text-left text-xs transition ${
                        selectedReasoningMode === DEFAULT_REASONING_MODE
                          ? 'bg-indigo-500/10 font-medium text-indigo-200'
                          : 'text-slate-300 hover:bg-[#2d2f31]'
                      }`}
                    >
                      <span>跟随配置</span>
                      {selectedReasoningMode === DEFAULT_REASONING_MODE && (
                        <span className="font-sans text-[10px] text-indigo-200">✓</span>
                      )}
                    </button>

                    {reasoningOptions.map((level) => (
                      <button
                        key={level}
                        onClick={(event) => {
                          event.stopPropagation();
                          onSelectReasoningMode(level);
                          setModelDropdownOpen(false);
                        }}
                        className={`flex cursor-pointer items-center justify-between rounded-xl px-2.5 py-1.5 text-left text-xs transition ${
                          selectedReasoningMode === level
                            ? 'bg-indigo-500/10 font-medium text-indigo-200'
                            : 'text-slate-300 hover:bg-[#2d2f31]'
                        }`}
                      >
                        <span>{formatReasoningLabel(level)}</span>
                        {selectedReasoningMode === level && (
                          <span className="font-sans text-[10px] text-indigo-200">✓</span>
                        )}
                      </button>
                    ))}
                  </div>
                </div>
              )}

              <div className="my-1 border-t border-[#2d2f31]/40" />

              {modelOptions.map((option) => {
                const isSelected = option.profileKey === selectedModel;

                return (
                  <div
                    key={option.profileKey}
                    className="relative rounded-2xl border border-transparent"
                  >
                    <button
                      onClick={() => {
                        onSelectModel(option.profileKey);
                        setModelDropdownOpen(false);
                      }}
                      onMouseEnter={() => handleMouseEnterRow(null)}
                      className={`flex w-full cursor-pointer items-center justify-between gap-3 rounded-2xl px-3 py-2.5 text-left transition hover:bg-[#2d2f31] ${
                        isSelected
                          ? 'border-indigo-500/20 bg-[#202224]'
                          : 'border-transparent'
                      }`}
                    >
                      <span className="min-w-0">
                        <span className="block truncate text-sm font-medium text-slate-200">
                          {option.modelId}
                        </span>
                        <span className="mt-0.5 block truncate text-[11px] text-slate-500">
                          {formatProviderModelLabel(option)}
                        </span>
                      </span>

                      {isSelected && (
                        <span className="rounded-full border border-indigo-500/20 bg-indigo-500/10 px-2 py-0.5 text-[11px] text-indigo-200 select-none">
                          已选
                        </span>
                      )}
                    </button>
                  </div>
                );
              })}
            </div>
          )}
        </div>

        <div className="h-full min-w-0 flex-1" />
      </div>
    </div>
  );
}
