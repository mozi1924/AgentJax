import { useEffect, useRef, useState } from 'react';
import { Menu, ChevronDown } from 'lucide-react';

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
  const dropdownRef = useRef(null);

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

  const selectedReasoningLabel = selectedModelOption?.supportsReasoning
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
            <div className="absolute left-0 top-10 z-50 w-[320px] rounded-2xl border border-[#2d2f31] bg-[#1e1f20] p-2 shadow-2xl">
              {modelOptions.map((option, index) => {
                const isSelected = option.profileKey === selectedModel;
                const reasoningOptions = Array.isArray(option.supportedReasoningLevels)
                  ? option.supportedReasoningLevels
                  : [];

                return (
                  <div
                    key={option.profileKey}
                    className={`${index > 0 ? 'mt-1.5' : ''} rounded-2xl border ${
                      isSelected ? 'border-cyan-400/30 bg-[#232526]' : 'border-transparent'
                    }`}
                  >
                    <button
                      onClick={() => {
                        onSelectModel(option.profileKey);
                      }}
                      className="flex w-full items-start justify-between gap-3 rounded-2xl px-3 py-2.5 text-left transition hover:bg-[#2d2f31]"
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
                        <span className="rounded-full border border-cyan-400/20 bg-cyan-400/10 px-2 py-0.5 text-[11px] text-cyan-200">
                          已选
                        </span>
                      )}
                    </button>

                    {isSelected && option.supportsReasoning && reasoningOptions.length > 0 && (
                      <div className="mx-2 mb-2 rounded-xl border border-[#2d2f31] bg-[#171819]/80">
                        <div className="px-3 pt-3 text-[11px] font-semibold uppercase tracking-[0.2em] text-slate-500">
                          推理等级
                        </div>
                        <div className="grid gap-1 p-2">
                          <button
                            onClick={() => onSelectReasoningMode(option.profileKey, DEFAULT_REASONING_MODE)}
                            className={`flex items-center justify-between rounded-xl px-3 py-2 text-left transition ${
                              selectedReasoningMode === DEFAULT_REASONING_MODE
                                ? 'bg-cyan-400/10 text-cyan-200'
                                : 'text-slate-300 hover:bg-[#2d2f31]'
                            }`}
                          >
                            <span className="text-sm">跟随配置</span>
                            <span className="text-[11px] text-slate-500">
                              {option.configuredReasoningEffort
                                ? `当前 ${formatReasoningLabel(option.configuredReasoningEffort)}`
                                : '未预设'}
                            </span>
                          </button>

                          {reasoningOptions.map((level) => (
                            <button
                              key={level}
                              onClick={() => onSelectReasoningMode(option.profileKey, level)}
                              className={`flex items-center justify-between rounded-xl px-3 py-2 text-left transition ${
                                selectedReasoningMode === level
                                  ? 'bg-cyan-400/10 text-cyan-200'
                                  : 'text-slate-300 hover:bg-[#2d2f31]'
                              }`}
                            >
                              <span className="text-sm">{formatReasoningLabel(level)}</span>
                              <span className="text-[11px] text-slate-500">覆盖本轮请求</span>
                            </button>
                          ))}
                        </div>
                      </div>
                    )}
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
