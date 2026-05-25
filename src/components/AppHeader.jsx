import { useEffect, useRef, useState } from 'react';
import { Menu, ChevronDown, ChevronRight } from 'lucide-react';

const DEFAULT_REASONING_MODE = '__default__';

const formatReasoningLabel = (value) => {
  const normalized = `${value || ''}`.trim().toLowerCase();
  if (!normalized || normalized === DEFAULT_REASONING_MODE) {
    return '跟随配置';
  }
  if (normalized === 'none') {
    return 'None';
  }
  return normalized.toUpperCase();
};

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
  cachePath
}) {
  const [modelDropdownOpen, setModelDropdownOpen] = useState(false);
  const [activeSubmenuKey, setActiveSubmenuKey] = useState(null);
  const dropdownRef = useRef(null);
  const submenuTimeoutRef = useRef(null);

  const handleMouseEnterRow = (key) => {
    if (submenuTimeoutRef.current) {
      clearTimeout(submenuTimeoutRef.current);
      submenuTimeoutRef.current = null;
    }
    setActiveSubmenuKey(key);
  };

  const handleMouseLeaveDropdown = () => {
    submenuTimeoutRef.current = setTimeout(() => {
      setActiveSubmenuKey(null);
    }, 150);
  };

  const handleMouseEnterSubmenu = () => {
    if (submenuTimeoutRef.current) {
      clearTimeout(submenuTimeoutRef.current);
      submenuTimeoutRef.current = null;
    }
  };

  useEffect(() => {
    return () => {
      if (submenuTimeoutRef.current) {
        clearTimeout(submenuTimeoutRef.current);
      }
    };
  }, []);

  useEffect(() => {
    const handlePointerDown = (event) => {
      if (!dropdownRef.current?.contains(event.target)) {
        setModelDropdownOpen(false);
      }
    };

    document.addEventListener('pointerdown', handlePointerDown);
    return () => {
      document.removeEventListener('pointerdown', handlePointerDown);
    };
  }, []);

  const hasReasoningSupport = !!selectedModelOption?.supportsReasoning;
  const reasoningOptions = hasReasoningSupport && Array.isArray(selectedModelOption?.supportedReasoningLevels)
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
              <span className="rounded-full border border-cyan-400/20 bg-cyan-400/10 px-2 py-0.5 text-[11px] text-cyan-200">
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
              {/* TOP ROW: Unified Global Reasoning Selection Row */}
              <div
                onMouseEnter={() => {
                  if (hasReasoningSupport) {
                    handleMouseEnterRow('reasoning');
                  }
                }}
                className={`relative flex w-full items-center justify-between gap-3 rounded-2xl px-3 py-2.5 text-left transition select-none ${
                  hasReasoningSupport
                    ? 'hover:bg-[#2d2f31] cursor-pointer group text-slate-200'
                    : 'opacity-45 cursor-not-allowed text-slate-400'
                }`}
              >
                <div className="flex-1 min-w-0">
                  <span className="block text-sm font-medium">
                    推理等级 (Reasoning)
                  </span>
                  <span className="mt-0.5 block text-[10px] text-slate-500 truncate">
                    {hasReasoningSupport
                      ? '选择全局思考努力程度'
                      : '当前选中模型不支持思考等级'}
                  </span>
                </div>
                {hasReasoningSupport && (
                  <div className="flex items-center gap-1.5 shrink-0 select-none text-xs text-cyan-300 font-medium">
                    <span>{formatReasoningLabel(selectedReasoningMode)}</span>
                    <ChevronRight className="h-3.5 w-3.5 text-slate-500 group-hover:text-slate-300 transition" />
                  </div>
                )}
              </div>

              {/* Secondary hover popout sub-menu for Global Reasoning Levels (Direct child of dropdown for pixel-perfect vertical alignment) */}
              {activeSubmenuKey === 'reasoning' && hasReasoningSupport && (
                <div
                  onMouseEnter={handleMouseEnterSubmenu}
                  className="absolute left-[324px] top-0 z-50 w-[180px] rounded-2xl border border-[#2d2f31] bg-[#1e1f20] p-2 shadow-2xl sidebar-context-menu before:absolute before:-left-3 before:top-0 before:bottom-0 before:w-3 before:content-['']"
                >
                  <div className="px-3 py-1.5 text-[11px] font-semibold uppercase tracking-[0.2em] text-slate-500 border-b border-[#2d2f31]/60 mb-1">
                    选择推理等级
                  </div>
                  <div className="grid gap-0.5">
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        onSelectReasoningMode(DEFAULT_REASONING_MODE);
                        setModelDropdownOpen(false);
                      }}
                      className={`flex items-center justify-between rounded-xl px-2.5 py-1.5 text-left transition text-xs cursor-pointer ${
                        selectedReasoningMode === DEFAULT_REASONING_MODE
                          ? 'bg-cyan-400/10 text-cyan-200 font-medium'
                          : 'text-slate-300 hover:bg-[#2d2f31]'
                      }`}
                    >
                      <span>跟随配置</span>
                      {selectedReasoningMode === DEFAULT_REASONING_MODE && (
                        <span className="text-[10px] text-cyan-200 font-sans">✓</span>
                      )}
                    </button>

                    {reasoningOptions.map((level) => (
                      <button
                        key={level}
                        onClick={(e) => {
                          e.stopPropagation();
                          onSelectReasoningMode(level);
                          setModelDropdownOpen(false);
                        }}
                        className={`flex items-center justify-between rounded-xl px-2.5 py-1.5 text-left transition text-xs cursor-pointer ${
                          selectedReasoningMode === level
                            ? 'bg-cyan-400/10 text-cyan-200 font-medium'
                            : 'text-slate-300 hover:bg-[#2d2f31]'
                        }`}
                      >
                        <span>{formatReasoningLabel(level)}</span>
                        {selectedReasoningMode === level && (
                          <span className="text-[10px] text-cyan-200 font-sans">✓</span>
                        )}
                      </button>
                    ))}
                  </div>
                </div>
              )}

              {/* Divider */}
              <div className="my-1 border-t border-[#2d2f31]/40" />

              {/* Models List */}
              {modelOptions.map((option, index) => {
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
                      className={`flex w-full items-center justify-between gap-3 rounded-2xl px-3 py-2.5 text-left transition hover:bg-[#2d2f31] cursor-pointer ${
                        isSelected ? 'border-cyan-400/30 bg-[#232526]' : 'border-transparent'
                      }`}
                    >
                      <span className="min-w-0">
                        <span className="block truncate text-sm font-medium text-slate-200">
                          {option.modelId}
                        </span>
                        <span className="mt-0.5 block truncate text-[11px] text-slate-500">
                          {option.providerKey || 'provider'} / {option.profileKey}
                        </span>
                      </span>

                      {isSelected && (
                        <span className="rounded-full border border-cyan-400/20 bg-cyan-400/10 px-2 py-0.5 text-[11px] text-cyan-200 select-none">
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
