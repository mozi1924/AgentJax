import { usePluginSettingsDataProvider } from './usePluginSettingsDataProvider';
import { useToolManagerDataProvider } from './useToolManagerDataProvider';
import type { SchemaDataProvider, SchemaDataProviderArgs } from './types';

// Registry boundary for built-in data providers. New providers should register
// here and expose data/actions by namespace, while SchemaRenderer remains generic.
export function useRegisteredSchemaDataProviders(
  args: SchemaDataProviderArgs
): SchemaDataProvider[] {
  const pluginProvider = usePluginSettingsDataProvider(args);
  const toolManagerProvider = useToolManagerDataProvider({
    ...args,
    search: args.queryState?.search,
    onSearchChange: args.queryState?.onSearchChange,
  });

  return [toolManagerProvider, pluginProvider];
}
