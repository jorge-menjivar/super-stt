# voices scope

> Scope: **voices** (add, list, play back, rename, and delete the recordings a
> cloned voice is built from).

The `voices` scope is the cloned-voice library. A `voices` token can upload a
reference recording, list what is stored, play a clip back, rename one, and
delete one. That is the whole surface: the library holds recordings and the
metadata describing them, and nothing else.

Speaking *in* a cloned voice needs only [`speak`](./speak.md), not this scope —
and nothing at all if the user has already selected it. The `voice:<uuid>` id is
set once with
[`POST /pipeline/{stage}/model/{model}/voice`](../endpoints/v1/pipeline/voice.md)
under [`settings`](./settings.md); from then on every utterance uses it, and a
speaking client never names it or touches the audio. The two are separate deliberately: an app that reads text aloud in a
voice the user already cloned has no reason to hold the recordings.

## Why it is not part of `settings`

Every other stored preference is a value the user typed. A voice sample is a
**recording of a person**, usually the user, and it is the one asset in the
daemon that identifies someone. The [`settings`](./settings.md) scope is broad
by design — a settings app holds it to manage models, devices, and backends —
and folding voice recordings into it would hand every such app the ability to
read them back as audio.

So the library gets its own scope, the consent prompt names it in those terms
("list, play back, and delete your saved voice recordings"), and a client that
wants both asks for both. Scopes never imply one another.

Clips are stored under `$XDG_DATA_HOME/super-tts/voices`, owner-readable only
(`0600`), and never leave the machine except to the loaded backend — which is
local unless the user selected an online model.

## Endpoint reference

| Endpoint                                                          | Methods                | Notes                                                     |
|-------------------------------------------------------------------|------------------------|-----------------------------------------------------------|
| [`/voice/list`](../endpoints/v1/voice.md)                         | GET                    | List the library.                                          |
| [`/voice`](../endpoints/v1/voice.md)                              | POST                   | Add a voice from a WAV upload.                             |
| [`/voice/{id}`](../endpoints/v1/voice.md)                         | GET, PATCH, DELETE     | Read, rename, or delete one voice.                        |
| [`/voice/{id}/audio`](../endpoints/v1/voice.md)                   | GET                    | The stored clip, as `audio/wav`.                          |

The library is the `/list` sub-resource of the singular noun, the same shape
every collection in the API takes: `/voice/{id}` is one voice, so `/voice/list`
is all of them, and there is no path that means both.

See [voice.md](../endpoints/v1/voice.md) for the full request/response shapes
and error model. Transport and framing are in [transport.md](../transport.md);
how scopes compose and how a token is obtained are in [auth.md](../auth.md).
