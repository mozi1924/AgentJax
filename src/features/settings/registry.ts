import { coreSettingsSchema } from './schema/coreSettingsSchema';
import { mcpRuntimeSettingsSchema } from './schema/mcpRuntimeSettingsSchema';
import { mcpServerSettingsSchema } from './schema/mcpServerSettingsSchema';
import { modelProfileSettingsSchema } from './schema/modelProfileSettingsSchema';
import { providerSettingsSchema } from './schema/providerSettingsSchema';
import type { SettingsModuleSchema, SettingsRegistry } from './types';

const builtinModules: SettingsModuleSchema[] = [
  coreSettingsSchema,
  providerSettingsSchema,
  modelProfileSettingsSchema,
  mcpRuntimeSettingsSchema,
  mcpServerSettingsSchema,
];

export const createSettingsRegistry = (): SettingsRegistry => {
  const sectionMap = new Map<string, SettingsRegistry['sections'][number]>();
  const fieldIds = new Set<string>();

  builtinModules.forEach((moduleSchema) => {
    moduleSchema.sections.forEach((section) => {
      const existing = sectionMap.get(section.id);
      if (!existing) {
        sectionMap.set(section.id, {
          ...section,
          children: [...section.children],
        });
        return;
      }

      existing.children.push(...section.children);
    });
  });

  const sections = Array.from(sectionMap.values()).sort((left, right) => left.order - right.order);

  const walk = (nodes: SettingsRegistry['sections'][number]['children']) => {
    nodes.forEach((node) => {
      if (fieldIds.has(node.id)) {
        throw new Error(`Duplicate settings schema node id: ${node.id}`);
      }
      fieldIds.add(node.id);
      if ('children' in node) {
        walk(node.children);
      }
    });
  };

  sections.forEach((section) => walk(section.children));
  return { sections };
};
