# Keeping the user informed

The user should never have to ask where things stand, and should never be surprised by a decision.

## Cadence

Default: images as work lands, without waiting for feedback (`cadence = as_work_lands` in `brief.toml`). Others: `per_batch`, `daily`,
`weekly`. Change it with `pxl decide "..." --set cadence=weekly --by user`. `pxl report --if-due` prints nothing when nothing is due.

## What a report holds (`pxl report [--html]`)

- done and in review, with the compare images of what is new;
- pending decisions, each with its options, images and your recommendation;
- budgets per scene and what changed;
- milestones: plan accepted, style approved, dry run passed;
- unconfirmed assumptions (so the user can correct them cheaply).

## Writing a decision for the user

One decision, three lines: what, the options with a picture each, your recommendation and why. Batch them. Example:

```
pxl ask "palette mood for the ice cave" \
  --option A=review/mood_a.png|source colours, slightly muted \
  --option B=review/mood_b.png|more vivid \
  --recommend A "closest to the source; B costs one identity colour"
```
When the user answers: `pxl decide d0001 A --by user`. Tasks that wait for it: `pxl task add ASSET "..." --after d0001`.

## Showing images

Send `compare.png` (input | latest) and `compare_1x.png` (the real size on the console's background). Send the sheets of the style
frames before production. Do not describe a picture you could send. If the user's tool cannot show images, put the paths in the report.
