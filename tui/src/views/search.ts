import {
  Box, Text, Input,
  BoxRenderable,
  TextRenderable,
  type CliRenderer,
  InputRenderableEvents,
} from "@opentui/core"
import { colors } from "../theme"
import { clearChildren } from "../helpers"
import * as himitsu from "../himitsu"

export function SearchView(renderer: CliRenderer, onAction: (action: string, data?: any) => void) {
  const resultsBox = new BoxRenderable(renderer, {
    id: "search-results",
    flexDirection: "column",
    flexGrow: 1,
  })

  function doSearch(query: string) {
    clearChildren(resultsBox)

    if (query.length < 2) {
      resultsBox.add(
        new TextRenderable(renderer, {
          id: "search-hint",
          content: "  Type at least 2 characters to search...",
          fg: colors.fgDim,
        }),
      )
      return
    }

    const results = himitsu.search(query, true)
    if (results.length === 0) {
      resultsBox.add(
        new TextRenderable(renderer, {
          id: "search-empty",
          content: `  No results for "${query}"`,
          fg: colors.fgDim,
        }),
      )
      return
    }

    resultsBox.add(
      new TextRenderable(renderer, {
        id: "search-count",
        content: `  ${results.length} result(s)`,
        fg: colors.fgDim,
        marginBottom: 1,
      }),
    )

    results.forEach((r, i) => {
      const row = new BoxRenderable(renderer, {
        id: `result-${i}`,
        flexDirection: "row",
        marginLeft: 2,
      })
      row.add(new TextRenderable(renderer, { id: `rk-${i}`, content: r.key.padEnd(24), fg: colors.fg }))
      row.add(new TextRenderable(renderer, { id: `re-${i}`, content: r.env.padEnd(12), fg: colors.yellow }))
      row.add(new TextRenderable(renderer, { id: `rr-${i}`, content: r.store, fg: colors.fgDim }))
      resultsBox.add(row)
    })
  }

  const searchInput = Input({
    placeholder: "Search secrets...",
    width: 40,
    backgroundColor: colors.bgDark,
    focusedBackgroundColor: colors.bgHighlight,
    textColor: colors.fg,
    cursorColor: colors.accent,
  })

  searchInput.on(InputRenderableEvents.INPUT, (value: string) => {
    doSearch(value)
  })

  searchInput.focus()
  doSearch("")

  return Box(
    { flexDirection: "column", flexGrow: 1 },
    Box(
      { flexDirection: "row", marginBottom: 1 },
      Text({ content: " Search", fg: colors.accent }),
    ),
    Box(
      { flexDirection: "row", marginBottom: 1, marginLeft: 2 },
      Text({ content: "/ ", fg: colors.accent }),
      searchInput,
    ),
    Box(
      { flexDirection: "column", marginLeft: 2, marginBottom: 1 },
      Text({ content: "KEY                     ENV         REMOTE", fg: colors.fgMuted }),
    ),
    resultsBox,
  )
}
