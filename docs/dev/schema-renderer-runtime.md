# Schema Renderer Runtime

The settings SchemaRenderer is the shared runtime for settings UI surfaces. Built-in
features and future plugins should describe UI with schema nodes, then expose data
and actions through a data provider namespace. They should not add feature-specific
React modules for custom settings panels.

## Runtime Model

A schema surface has three parts:

- `SettingsSchemaNode[]`: declarative layout, fields, lists, detail panels, actions,
  and bindings.
- `SchemaDataProvider`: a namespaced data/action provider such as `toolManager`.
- `SchemaRenderer`: the shared renderer that applies the app's settings modal style.

Plugins can contribute static settings sections through manifest
`settingsSections`; those sections are merged into the settings UI snapshot by
the backend. Dynamic plugin sections use the same schema nodes plus a provider
namespace.

The renderer routes data by namespace. For example, `toolManager.tools` is handled
by the provider whose namespace is `toolManager`. A future plugin provider can use
the same contract with names such as `plugin.demo.items`.

Plugins can also declare simple manifest-backed dynamic data with `settingsData`.
Relative keys are exposed as `plugin.{pluginId}.{key}`. Fully qualified keys that
start with `plugin.` are used as-is.

```json
{
  "settingsData": {
    "items": [
      {
        "id": "primary",
        "name": "Primary item",
        "description": "Rendered by the shared plugin data provider."
      }
    ]
  }
}
```

## Data Source Nodes

Use data-source UI nodes when the UI is backed by dynamic data rather than direct
settings fields.

```json
{
  "kind": "list",
  "id": "plugin-items",
  "dataSource": "plugin.demo.items",
  "action": "selectItem",
  "emptyText": "settings.plugin.empty_items",
  "itemTemplate": {
    "kind": "detail",
    "id": "plugin-item-template",
    "bindings": {
      "id": "id",
      "activeKey": "activeItemId",
      "title": "name",
      "description": "description"
    },
    "actions": [
      {
        "id": "toggleItem",
        "variant": "switch",
        "path": "policyPath",
        "value": "enabled",
        "savingKey": "savingKey"
      }
    ]
  }
}
```

For list/detail pairs backed by the generic plugin provider, point the list at
the array source and the detail panel at the same source with `.selected`:

```json
{
  "kind": "detail",
  "id": "plugin-selected-item",
  "dataSource": "plugin.demo.items.selected",
  "emptyText": "settings.plugin.empty_selection",
  "itemTemplate": {
    "kind": "detail",
    "id": "plugin-selected-item-template",
    "bindings": {
      "title": "name",
      "description": "description"
    }
  }
}
```

The generic plugin provider uses
`src/components/settings/schemaRenderer/dataSources/pluginSettingsData.ts` for
manifest-backed list filtering, active-row hydration, and `.selected` detail
resolution. Keep new plugin data conventions in that runtime so they can be
tested without introducing plugin-specific React modules.

Supported structural nodes include `layout`, `split`, `tabs`, `toolbar`, `list`,
`panel`, `detail`, and `collapsible`. Display/action nodes include `badge`,
`metric`, `empty_state`, and `action`. Static settings fields still use `field`,
`group`, and `collection`.

`layout` is the generic composition container. Use `layout: "stack"` or omit
`layout` for vertical sections, `layout: "inline"` for compact tool rows, and
`layout: "grid"` for summary metrics or mixed display controls. The same layout
classes are used for static settings nodes and provider-backed plugin panels.

```json
{
  "kind": "layout",
  "id": "plugin-summary-grid",
  "layout": "grid",
  "children": [
    { "kind": "metric", "id": "enabled-count", "dataSource": "plugin.demo.summary" },
    { "kind": "badge", "id": "runtime-status", "dataSource": "plugin.demo.summary" }
  ]
}
```

`split` is a generic responsive layout container. Use `layout: "two-pane"` for
list/detail or navigation/detail plugin panels, `layout: "three-pane"` for
Tools-Manager-style navigation/list/detail surfaces, or omit `layout` for an
auto-fitting split. The renderer applies the shared settings modal style for
all variants.

```json
{
  "kind": "split",
  "id": "plugin-runtime-split",
  "layout": "two-pane",
  "children": [
    { "kind": "list", "id": "plugin-items", "dataSource": "plugin.demo.items" },
    { "kind": "detail", "id": "plugin-detail", "dataSource": "plugin.demo.items.selected" }
  ]
}
```

Use `defaultExpanded` on `collapsible` nodes or detail templates to control the
initial collapsed state without adding feature-specific frontend state.

```json
{
  "kind": "collapsible",
  "id": "advanced-runtime",
  "title": "settings.plugin.advanced_runtime",
  "defaultExpanded": false,
  "children": []
}
```

Display/action nodes can bind directly to provider data by setting `dataSource`
and `bindings`, so plugins can render status summaries without React components.

```json
[
  {
    "kind": "metric",
    "id": "plugin-enabled-count",
    "dataSource": "plugin.demo.summary",
    "bindings": {
      "title": "label",
      "value": "enabledCount",
      "description": "description"
    }
  },
  {
    "kind": "badge",
    "id": "plugin-status",
    "dataSource": "plugin.demo.summary",
    "bindings": { "value": "status" }
  },
  {
    "kind": "empty_state",
    "id": "plugin-empty-state",
    "dataSource": "plugin.demo.empty",
    "bindings": {
      "title": "title",
      "description": "description"
    }
  },
  {
    "kind": "action",
    "id": "plugin-refresh",
    "dataSource": "plugin.demo.summary",
    "title": "settings.plugin.refresh",
    "icon": "RefreshCcw",
    "variant": "button"
  }
]
```

## Bindings

Bindings are dot paths read from the provider's item data. Common bindings:

- `id`: stable key for list or tab rows.
- `activeKey`: key compared to `id` for active row styling.
- `title`: primary text.
- `description`: secondary body text.
- `meta`, `secondaryMeta`, `count`: compact metadata joined with separators.
- `badge`: compact badge in detail headers.
- `value`: value used by `badge`, `metric`, and action controls.
- `label`: label used by `metric`.
- `detailItems`: array path rendered as repeated detail rows.
- `detailItemsTitle`: heading for `detailItems`.
- `detailItemsEmptyText`: empty state for `detailItems`.
- `detailItemName`, `detailItemType`, `detailItemDescription`, `detailItemRequired`:
  item paths used by generic detail item rows.

## Properties

`properties` render compact labeled facts in toolbars and detail templates.

```json
{
  "properties": [
    {
      "id": "model-name",
      "label": "settings.tools.property.model",
      "value": "modelName",
      "variant": "code"
    },
    {
      "id": "schema-format",
      "label": "settings.tools.property.schema_format",
      "value": "schemaFormat",
      "variant": "badge"
    }
  ]
}
```

Supported property variants are `text`, `code`, `badge`, and `status`. `visibleWhen`
uses the same condition shape as settings fields, evaluated against the provider
item.

## Actions

Actions are dispatched to the provider selected by `dataSource`. Current variants:

- `button`: command button; supports Lucide `icon`.
- `switch`: boolean toggle.
- `segmented`: compact option picker.
- `select`: native select option picker.
- `search`: scoped search input for provider-owned search surfaces.

Actions support `visibleWhen` and `disabledWhen` with the same condition shape as
settings fields. For data-source nodes those conditions are evaluated against the
current provider item, so a schema can hide a switch when the item has no
`policyPath`, or disable a command when a provider marks the item as locked.

Global settings search is owned by the settings modal and passed into providers,
so feature-specific search controls should only be used when the surface needs a
separate scoped query.

The modal uses two related search passes. Section navigation matches section
schema text plus a lightweight dynamic namespace index for Tool Manager and
plugin settings snapshots. The active section render pass preserves data-source
surfaces so providers can filter their rows at runtime. This keeps dynamic
sections discoverable for matching tool/plugin data without showing every
`dataSource` section for unrelated queries.

## Provider Contract

Providers implement:

- `namespace`: namespace prefix used for data-source routing.
- `enabled`: whether the current schema uses this provider.
- `getDataSource(dataSource)`: returns dynamic data for a schema node.
- `dispatch(action, payload)`: handles schema actions.
- `getStatus(dataSource)`: optional loading and error state.
- `isSaving(savingKey)`: optional saving state for row controls.

Register built-in providers in
`src/components/settings/schemaRenderer/dataSources/providerRegistry.ts`. The
runtime derives `requestedDataSourceNamespaces` from the active schema and passes
that list to providers, so a provider can cheaply return `enabled: false` when
its namespace is absent.

Shared data-source visual primitives live in
`src/components/settings/schemaRenderer/dataSources/ui.tsx`. New data-source
surfaces should reuse those primitives for switches, buttons, segmented controls,
search inputs, badges, and property grids so plugin-rendered panels match the
settings modal instead of introducing feature-specific styling.
