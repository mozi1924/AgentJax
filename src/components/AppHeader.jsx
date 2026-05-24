import { useEffect, useRef, useState } from 'react';
import { Menu, ChevronDown } from 'lucide-react';

export default function AppHeader({
  titlebarRef,
  sidebarOpen,
  onToggleSidebar,
  selectedModel,
  modelOptions,
  onSelectModel,
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
            <span className="truncate">AgentJax {selectedModel}</span>
            <ChevronDown className="h-3 w-3 text-slate-400" />
          </button>

          {modelDropdownOpen && (
            <div className="absolute left-0 top-10 z-50 w-56 rounded-2xl border border-[#2d2f31] bg-[#1e1f20] p-2 shadow-2xl">
              {modelOptions.map((model, index) => (
                <button
                  key={model}
                  onClick={() => {
                    onSelectModel(model);
                    setModelDropdownOpen(false);
                  }}
                  className={`flex w-full flex-col rounded-xl px-3 py-2 text-left transition hover:bg-[#2d2f31] ${index > 0 ? 'mt-1' : ''}`}
                >
                  <span className="text-sm font-medium text-slate-200">{model}</span>
                </button>
              ))}
            </div>
          )}
        </div>

        <div className="h-full min-w-0 flex-1" />
      </div>
    </div>
  );
}
