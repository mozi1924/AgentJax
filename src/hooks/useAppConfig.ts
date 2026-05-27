import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  buildFallbackModelOption,
  DEFAULT_MODEL_PROFILE,
  DEFAULT_REASONING_MODE,
  normalizeModelOption,
  resolveConfiguredDefaultOptionProfileKey,
} from '../features/models/modelCatalog';
import type { ModelCatalogResponse, ModelOption } from '../features/conversations/types';
import type { SettingsSnapshot, SettingsSnapshotEvent } from '../features/settings/types';
import { tryGetCurrentWindow } from '../features/tauri/runtime';

const isModelOption = (option: ModelOption | null): option is ModelOption =>
  option !== null;

const resolveShowAdvancedRequestOptionsButton = (
  values: Record<string, unknown> | undefined
): boolean => values?.show_advanced_request_options === true;

export function useAppConfig() {
  const [selectedModel, setSelectedModel] = useState(DEFAULT_MODEL_PROFILE);
  const [modelOptions, setModelOptions] = useState<ModelOption[]>([]);
  const [selectedReasoningMode, setSelectedReasoningMode] = useState(DEFAULT_REASONING_MODE);
  const [configPath, setConfigPath] = useState('');
  const [cachePath, setCachePath] = useState('');
  const [showAdvancedRequestOptionsButton, setShowAdvancedRequestOptionsButton] = useState(false);

  const selectedModelRef = useRef(DEFAULT_MODEL_PROFILE);
  const selectedReasoningModeRef = useRef(DEFAULT_REASONING_MODE);

  const selectedModelOption = useMemo(
    () =>
      modelOptions.find((option) => option.profileKey === selectedModel) ||
      modelOptions[0] ||
      null,
    [modelOptions, selectedModel]
  );

  useEffect(() => {
    selectedModelRef.current = selectedModel;
  }, [selectedModel]);

  useEffect(() => {
    selectedReasoningModeRef.current = selectedReasoningMode;
  }, [selectedReasoningMode]);

  const refreshModelCatalog = useCallback(async () => {
    const catalog = await invoke<ModelCatalogResponse>('get_model_catalog');
    if (!catalog) return;

    const available =
      Array.isArray(catalog.modelOptions) && catalog.modelOptions.length > 0
        ? catalog.modelOptions.map(normalizeModelOption).filter(isModelOption)
        : (
            Array.isArray(catalog.effectiveModels) && catalog.effectiveModels.length > 0
              ? catalog.effectiveModels
              : [DEFAULT_MODEL_PROFILE]
          ).map(buildFallbackModelOption);
    const configuredDefault = (catalog.defaultModel || '').trim();
    const configuredDefaultProfileKey = resolveConfiguredDefaultOptionProfileKey(
      configuredDefault,
      available
    );
    const preservedSelection = available.find(
      (option) => option.profileKey === selectedModelRef.current
    )?.profileKey;
    const nextModel =
      preservedSelection ||
      configuredDefaultProfileKey ||
      available[0]?.profileKey ||
      DEFAULT_MODEL_PROFILE;
    const nextModelOption =
      available.find((option) => option.profileKey === nextModel) || null;
    const preservedReasoning = selectedReasoningModeRef.current;
    const canPreserveReasoning =
      !!nextModelOption?.supportsReasoning &&
      preservedReasoning !== DEFAULT_REASONING_MODE &&
      nextModelOption.supportedReasoningLevels.includes(preservedReasoning);

    setModelOptions(available);
    setSelectedModel(nextModel);
    setSelectedReasoningMode(
      canPreserveReasoning
        ? preservedReasoning
        : nextModelOption?.configuredReasoningEffort || DEFAULT_REASONING_MODE
    );
    if (catalog.configPath) {
      setConfigPath(catalog.configPath);
    }
    if (catalog.cachePath) {
      setCachePath(catalog.cachePath);
    }
  }, []);

  useEffect(() => {
    let mounted = true;

    refreshModelCatalog().catch(() => {
      if (!mounted) {
        return;
      }
      // Keep frontend defaults when backend config cannot be loaded.
    });

    return () => {
      mounted = false;
    };
  }, [refreshModelCatalog]);

  useEffect(() => {
    let mounted = true;

    invoke<SettingsSnapshot>('get_settings_snapshot')
      .then((snapshot) => {
        if (!mounted) {
          return;
        }
        setShowAdvancedRequestOptionsButton(
          resolveShowAdvancedRequestOptionsButton(snapshot.values)
        );
      })
      .catch(() => {});

    return () => {
      mounted = false;
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    const setup = async () => {
      const currentWindow = tryGetCurrentWindow();
      if (!currentWindow) {
        return;
      }

      unlisten = await currentWindow.listen<SettingsSnapshotEvent>(
        'config_snapshot_changed',
        (event) => {
          if (disposed) return;
          setShowAdvancedRequestOptionsButton(
            resolveShowAdvancedRequestOptionsButton(event.payload.values)
          );
          void refreshModelCatalog().catch(() => {});
        }
      );

      if (disposed && unlisten) {
        unlisten();
        unlisten = null;
      }
    };

    void setup();

    return () => {
      disposed = true;
      if (unlisten) {
        unlisten();
        unlisten = null;
      }
    };
  }, [refreshModelCatalog]);

  const selectReasoningMode = useCallback((reasoningMode: string) => {
    setSelectedReasoningMode(reasoningMode || DEFAULT_REASONING_MODE);
  }, []);

  return {
    cachePath,
    configPath,
    modelOptions,
    refreshModelCatalog,
    selectedModel,
    selectedModelOption,
    selectedReasoningMode,
    selectReasoningMode,
    setSelectedModel,
    showAdvancedRequestOptionsButton,
  };
}
