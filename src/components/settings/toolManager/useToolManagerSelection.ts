import { useEffect, useMemo, useState } from 'react';
import {
  filterToolsForQuery,
  selectToolManagerSource,
  sourceIdentityKey,
  sourcesForCategory,
  type ToolCategory,
  type ToolManagerSnapshot,
  type ToolManagerSourceSnapshot,
} from '../../../features/settings/toolManagerView';

// Keeps category/source/tool/search state separate from snapshot loading and policy actions.
export function useToolManagerSelection(snapshot: ToolManagerSnapshot | null) {
  const [activeCategory, setActiveCategory] = useState<ToolCategory>('native');
  const [selectedSourceKey, setSelectedSourceKey] = useState('');
  const [selectedToolId, setSelectedToolId] = useState('');
  const [search, setSearch] = useState('');

  const categorySources = useMemo(() => {
    const sources = snapshot?.sources || [];
    return sourcesForCategory(sources, activeCategory);
  }, [activeCategory, snapshot]);

  const activeSource = useMemo(
    () => selectToolManagerSource(categorySources, selectedSourceKey),
    [categorySources, selectedSourceKey]
  );

  const filteredTools = useMemo(
    () => filterToolsForQuery(activeSource?.tools || [], search),
    [activeSource, search]
  );

  const selectedTool = useMemo(
    () => filteredTools.find((tool) => tool.id === selectedToolId) || filteredTools[0] || null,
    [filteredTools, selectedToolId]
  );

  useEffect(() => {
    if (!selectedTool && filteredTools[0]) {
      setSelectedToolId(filteredTools[0].id);
      return;
    }
    if (selectedTool) {
      setSelectedToolId(selectedTool.id);
    }
  }, [filteredTools, selectedTool]);

  const selectCategory = (category: ToolCategory) => {
    setActiveCategory(category);
    setSelectedSourceKey('');
    setSelectedToolId('');
    setSearch('');
  };

  const selectSource = (source: ToolManagerSourceSnapshot) => {
    setSelectedSourceKey(sourceIdentityKey(source));
    setSelectedToolId('');
    setSearch('');
  };

  return {
    activeCategory,
    categorySources,
    activeSource,
    filteredTools,
    selectedTool,
    selectedToolId,
    search,
    setSearch,
    setSelectedToolId,
    selectCategory,
    selectSource,
  };
}
