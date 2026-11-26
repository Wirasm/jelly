# compare

**Use when** the person asked for a prototype, a spike or a few takes on one
feature, and has to choose between them by using them. It shows the versions
side by side in one page: they switch version, walk each flow step by step,
comment on any part of any step, and pick one. It answers "which of these, and
what would I change in it?"

```bash
# Write your own state.json. To change the frame: jelly recipes compare --out DIR
jelly make invite --recipe compare --state state.json --about "Invite flow: three takes"
jelly asset invite path/to/the/products/app.css      # then name it in state.css
jelly look invite                             # fix every `!` and `error` line; look again on a `failed` one
jelly try invite --click "[data-version=sheet]" --click ".jc-screen:not([hidden]) [data-next]"
jelly show invite
jelly hear --wait invite                      # in the background: wakes you when they pick
```

The example `state.json` and `assets/example.css` are a made-up product. Replace
both. Only the frame is jelly's: the bar on top and the rules for walking a flow.

## The one rule: the screens are the product, not jelly

What is under the bar must look like the product the person is building, not
like a jelly recipe and not like your own taste.

- **Use the project's own stylesheets.** Add the built CSS as an asset and list
  it in `css`. If the project has a DESIGN.md, a token file or a component
  library, read it before you write a step.
- **Use the project's own markup.** Copy the class names and structure its real
  components produce. A button is the product's button.
- **No library of jelly components here.** The frame knows nothing about looks
  and gives the screens no CSS.

## State

| Key | What goes in it |
| --- | --- |
| `title` | The feature, in a few words. |
| `question` | What they are choosing, as a question. Shown in the bar. |
| `css` | Stylesheets for the screens, in order: `assets/<file>` or a URL the person's browser can reach. |
| `versions[]` | `id` (lowercase, digits, dashes), `label` (two or three words), `note` (one line: what is different about this one), `steps[]`. |
| `versions[].steps[]` | `label` (one word) and `html`: that screen, as the product would render it. |
| `recommend`, `why` | The version you would pick, and one line on why. Leave both out if you have no view. |
| `picked` | Written by `pick`: `{"id", "by", "at_ms"}`. |

## Walking a flow

Inside a step's HTML, mark the controls that move the flow:

- `data-next` goes to the next step, `data-back` to the one before.
- `data-go="3"` goes to step 3.

Everything else in a screen is inert: links and forms go nowhere. Steps are
HTML, not scripts: a `<script>` in a step does not run. For behaviour inside one
screen, change the view (`jelly recipes compare --out DIR`).

Do not put `id` attributes in step HTML. Each screen sits in the page under the
id `<version>-<step>` (`sheet-2`), and a comment's selector starts from the
nearest id. That is how a comment tells you which version and step it is about.

## What you hear

- `pick {"id":"sheet"}` when they choose. The view also sends, so a
  `jelly hear --wait` in the background wakes you.
- Comments, each with a selector like `#sheet-2 > div > button`: version
  `sheet`, step 2, that button.

Change a version while they look with `jelly set invite --at
/versions/1/steps/0/html @step.html`: the person stays on the version and step they were on.

## Verbs

- `pick {"id"}`: the person's choice.
- `unpick`: they changed their mind.
