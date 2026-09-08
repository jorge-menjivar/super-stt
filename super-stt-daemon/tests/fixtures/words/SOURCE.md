# English word frequency list

`en_10k.txt` is the first 10,000 lines of `content/2018/en/en_50k.txt` from
<https://github.com/hermitdave/FrequencyWords>, downloaded on 2026-09-08 from
commit `072bbed282316a23651aa7068c7173aa7898cf80`. Each line is a word and its count in the
OpenSubtitles2018 corpus (<http://opus.nlpl.eu/OpenSubtitles2018.php>), most
frequent first. The file is unmodified apart from the truncation.

## License

The repository's README states "MIT License for code. CC-by-sa-4.0 for
content." This file is content, so it is used under the Creative Commons
Attribution-ShareAlike 4.0 International license
(<https://creativecommons.org/licenses/by-sa/4.0/>). Attribution: Hermit Dave,
FrequencyWords, from OpenSubtitles2018.

## Use

`super-stt-daemon/src/output/stitch_simulation_tests.rs` samples words by
their counts to generate the passages it cuts into windows and stitches. Words
that are not purely alphabetic (the contraction pieces `'s` and `'t`) are
skipped when the list is loaded.
