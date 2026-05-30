import type { ToolManagerToolSnapshot } from '../../../features/settings/toolManagerView';

const asRecord = (value: unknown): Record<string, unknown> => {
  if (!value) return {};
  if (typeof value === 'string') {
    try {
      const parsed = JSON.parse(value);
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
        return parsed as Record<string, unknown>;
      }
    } catch {
      // ignore
    }
  }
  return typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
};

const asStringArray = (value: unknown): string[] =>
  Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];

const schemaTypeLabel = (schema: Record<string, unknown>) => {
  const enumValues = asStringArray(schema.enum);
  let type = 'value';
  if (typeof schema.type === 'string') {
    type = schema.type;
  } else if (Array.isArray(schema.type)) {
    type = schema.type.filter((t): t is string => typeof t === 'string').join(' | ');
  }
  return enumValues.length > 0 ? `${type} enum(${enumValues.join(', ')})` : type;
};

// Renders the complete input schema returned by the backend while keeping list cards lightweight.
export function ToolSchemaPreview({ tool }: { tool: ToolManagerToolSnapshot }) {
  const schema = asRecord(tool.inputSchema);
  const properties = asRecord(schema.properties);
  const required = new Set(asStringArray(schema.required));
  const entries = Object.entries(properties);

  if (entries.length === 0) {
    return (
      <div className="rounded-lg border border-dashed border-[#2b2b2d] px-3 py-4 text-center text-[11px] text-neutral-500">
        No parameters
      </div>
    );
  }

  return (
    <div className="space-y-2">
      {entries.map(([name, property]) => {
        const propertyRecord = asRecord(property);
        const description =
          typeof propertyRecord.description === 'string' ? propertyRecord.description : '';
        return (
          <div key={name} className="rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/20 px-3 py-2">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-mono text-[11px] text-neutral-200">{name}</span>
              <span className="rounded bg-[#2a2a2c] px-1.5 py-0.5 text-[10px] text-neutral-300">
                {schemaTypeLabel(propertyRecord)}
              </span>
              {required.has(name) && (
                <span className="rounded bg-amber-500/10 px-1.5 py-0.5 text-[10px] text-amber-200">
                  required
                </span>
              )}
            </div>
            {description && (
              <p className="mt-1 line-clamp-3 text-[11px] leading-relaxed text-neutral-500">
                {description}
              </p>
            )}
          </div>
        );
      })}
    </div>
  );
}
