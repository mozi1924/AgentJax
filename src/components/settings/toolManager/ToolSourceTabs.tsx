import { useI18n } from '../../../features/i18n';
import {
  TOOL_MANAGER_CATEGORIES,
  type ToolCategory,
} from '../../../features/settings/toolManagerView';

export function ToolSourceTabs({
  activeCategory,
  onSelectCategory,
}: {
  activeCategory: ToolCategory;
  onSelectCategory: (category: ToolCategory) => void;
}) {
  const { t } = useI18n();

  return (
    <div className="flex flex-wrap items-center gap-1 border-b border-[#242426] px-3 py-2">
      {TOOL_MANAGER_CATEGORIES.map((category) => (
        <button
          key={category.id}
          type="button"
          onClick={() => onSelectCategory(category.id)}
          className={`rounded-md px-2.5 py-1 text-[12px] transition ${
            activeCategory === category.id
              ? 'bg-[#2a2a2c] text-white'
              : 'text-neutral-400 hover:bg-[#202022] hover:text-white'
          }`}
        >
          {t(category.labelKey)}
        </button>
      ))}
    </div>
  );
}
