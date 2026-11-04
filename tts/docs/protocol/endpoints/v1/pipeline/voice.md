# `/pipeline/{stage}/model/{model}/voice`

Read and set a model's **voice** — the one it speaks in when an utterance names
none — and read which voices it could be set to. Stored per `(source, model)`
and survives model switches.

Addressed through the stage that runs the model, exactly like its
[device](./device.md) and [language](./language.md), and for the same reason:
all three are per-model preferences, and the stage is what resolves a bare model
name against the backend filling it. Three preferences of the same shape
addressed three different ways is something a client author has to memorise
rather than infer.

The symmetry goes one level further. What is *set* and what may *be set* are
separate endpoints, as they are for the device and the language:

| | is set | may be set |
|---|---|---|
| device   | [`/device`](./device.md#get-pipelinestagemodelmodeldevice)     | [`/device/list`](./device.md#get-pipelinestagemodelmodeldevicelist)     |
| language | [`/language`](./language.md)                                   | [`/language/list`](./language.md#get-pipelinestagemodelmodellanguagelist) |
| voice    | `/voice`                                                       | [`/voice/list`](#get-pipelinestagemodelmodelvoicelist)                  |

```
/pipeline/1/model/qwen3-tts-0.6b-custom-voice/voice
```

## Why this is per model

A voice id means nothing to another model. `ryan` is one of the nine speakers
the Qwen CustomVoice checkpoints declare and is not a voice any Kokoro build
has; a cloned `voice:<uuid>` is refused outright by a model whose `voice_kinds`
does not include `cloned`. One global voice setting would be wrong for every
model but the one it was picked for, which is why this is keyed the same way the
device and the language are.

## Relationship to `POST /speak`

**This is the only place a voice is chosen.** [`POST /speak`](../speak.md) takes
`text` and nothing else, so what is stored here is what every caller gets: a
keyboard shortcut, the panel applet, the settings app's Speak button, a browser
client. Speaking and configuring are separate grants, and an app that can make
the machine talk does not thereby get a say in which voice it talks in.

`voice` used to be a per-utterance field on `/speak`, and one named there won.
Two consequences of removing it are worth knowing. Auditioning a clip is now a
selection — the Voices page sets the voice and then speaks, so the preview a user
hears is the voice they will get everywhere afterwards. And a model with no
stored voice whose voices are all cloned has no way to be given one per request,
so it answers *"this model speaks in a cloned voice, so a request has to name
one"* until a voice is stored here.

**Both speak paths fall back to it.** `POST /speak` and the `start` frame of
[`GET /speak/stream`](../speak/stream.md) resolve it the same way. They did not
always: the streaming path handed its frame straight to the speech engine, so a
`start` naming no voice reached the backend with none and a clone-only model
refused every streamed utterance with the message above — while the stored voice
sat unread. A daemon-side contract test now fails if an HTTP path opens an
utterance without going through the resolver.

## `GET /pipeline/{stage}/model/{model}/voice`

The voice in effect and what decided it.

```json
{
  "status": "success",
  "voice": {
    "effective": "ryan",
    "source": "default",
    "override": null,
    "default": "ryan",
    "kinds": ["preset"]
  }
}
```

| Field | Meaning |
|---|---|
| `effective` | What an utterance naming no voice is spoken in. `null` when the model has neither a stored voice nor a `default_voice`. |
| `source`    | `override` when a voice is stored, `default` otherwise. |
| `override`  | The stored voice, or `null`. |
| `default`   | The manifest's `default_voice`, or `null` when it declares none. |
| `kinds`     | The id shapes the model accepts: `preset`, `cloned`, `described`. |

**A `null` `effective` is not a quirk.** It is the state in which the model
cannot speak: no id to fall back to, and every utterance refused until one is
set. A client should render it as a control the user must fill in, not as an
optional one. It is the normal starting state for a cloning model, whose voices
do not exist until the user records one.

`kinds` is how a client tells two empty lists apart. A model that accepts
`described` voices takes free text — there is nothing to enumerate and nothing
is wrong; offer a text field whose value is sent as `desc:<text>`. A model that
clones and lists nothing has no voice at all yet.

Answers whether or not the model is loaded, which is how a card shows its voice
before Load.

## `POST /pipeline/{stage}/model/{model}/voice`

```json
{ "voice": "ryan" }
```

Stores the voice. A voice the model cannot resolve is refused rather than
stored — a shape its `voice_kinds` does not include, or a preset id it does not
declare — because the alternative is a setting that looks accepted and then
fails on every utterance that follows. Offer what
[`/voice/list`](#get-pipelinestagemodelmodelvoicelist) returns, plus a free-text
`desc:<text>` when `kinds` includes `described`.

An empty string is `400 invalid_request`: clearing is `DELETE`, which is a
different act and answers differently.

The new value applies to the next utterance; one already being spoken finishes
in the voice it started in. Answers with the resolution that results, so a card
can render its own click without a second read.

## `DELETE /pipeline/{stage}/model/{model}/voice`

Removes the stored voice, returning the model to its manifest `default_voice`.
Answers with the same block `GET` does — `source` will be `default`, and
`effective` will be `null` for a model that declares no default, which is again
the state where speaking is refused until a voice is chosen.

## `GET /pipeline/{stage}/model/{model}/voice/list`

What `POST` will accept: the model's presets, then the stored cloned voices when
its `voice_kinds` includes `cloned`.

```json
{
  "status": "success",
  "available_voices": [
    { "id": "ryan", "label": "Ryan (English, male)", "kind": "preset" },
    { "id": "voice:0b2f8c1e-…", "label": "My voice", "kind": "cloned" }
  ]
}
```

Fill a picker from this rather than from the manifest alone. A cloning model
declares no `[[models.voices]]`, so a picker built from the manifest would be
empty for exactly the models that refuse to speak without a voice; and a cloned
voice belongs to the [library](../voice.md), not to any one model, so the
manifest could not list it either.

Described voices are not here — free text has no set to enumerate. `kinds` on
the block above is what says a model takes them.

An empty list with no `described` kind means the model has no voices to choose
yet: record one on [`POST /voice`](../voice.md#post-voice) first.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.

## Errors

| Status | `message` | When |
|---|---|---|
| 400 | `invalid_request` | `voice` was empty on `POST`. |
| 400 | `invalid_value`   | The voice is not one this model can resolve. |
| 400 | `invalid_backend` | The stage has no backend selected. |
| 404 | `unknown_stage`   | No such pipeline stage. |
| 404 | `unknown_model`   | This stage's backend serves no such model. |
