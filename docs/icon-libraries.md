# UI icon libraries

Lizard ships an offline, versioned SVG catalog for six icon families: Lucide, Feather,
Phosphor, and Material Symbols Outlined, Rounded and Sharp. The catalog is compiled from pinned
redistributable packages by `scripts/build-icon-catalog.py`. Package versions, archive hashes
and licences live in `assets/icon-packs/` and `NOTICE`. Rendering never depends on a
network request or on the agent provider.

## Agent workflow

The MCP tool `search_icons` accepts `query`, optional `preferredLibrary` (default Lucide) and `limit`. When the
preferred family has a name match, results come only from that family. The tool searches the
others when there is no match and reports `usedFallback: true`. A screen's most-used placed
icon family is supplied in each in-app agent snapshot as `preferred_icon_library`; a new screen
defaults to Lucide. The agent harness and MCP instructions ask agents to keep that family across
a screen unless it lacks a suitable icon.

After searching, create a layer with `create_node` using `type: "icon"`, `library` and the exact
`icon` name returned by the search. The engine also exposes `icon.create` and accepts the same
fields through `node.create`. Example:

```json
{"type":"icon","library":"lucide","icon":"home","parent":123,"x":16,"y":16,"width":24,"color":"#334155"}
```

An icon is an editable frame containing vector paths. Its root stores
`lizard.icon.library`, `lizard.icon.name` and `lizard.icon.color` in `pluginData`. The rendered geometry is saved in
the document, so old files keep drawing correctly even after a catalog update. The frame and
its path children participate in the normal command, undo, copy and serialization flows. One
requested width or height preserves the original aspect ratio. SVG imports are recolored to the
requested colour; their transparency is retained.

Use the command `icon.set {id, library?, icon?, color?, width?, height?}` to replace or recolor a
placed icon while retaining its layer ID, position and undo history. The library and icon name
must be an exact catalog pair.

## Updating the catalog

Update the pinned package versions in `scripts/build-icon-catalog.py`, run the script from the
repository root, inspect the resulting counts and licences, and update `NOTICE`. The
generator accepts locally cached archives named `/tmp/lizard-icons-<source>.tgz` or downloads
the pinned archives. Run the engine icon catalog tests and the MCP icon creation test; visually
inspect an exported icon before shipping.

The assets use static SVG outlines. Material Symbols' variable font axes and Phosphor's
duotone palette are not exposed as editor controls; the catalog provides Material's static
weight 400 variants and Phosphor's named SVG weights. A placed icon can be edited as vectors,
while its metadata identifies the original family and name for future tooling.
