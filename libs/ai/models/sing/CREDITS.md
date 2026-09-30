# Credits: data used to train the Cantor singing voice

Cantor's code is our own. The voice weights are trained on the datasets below. Each is used under its
licence, and each entry says how we modified it. A shipped voice asset carries this text in its
metadata (`credits`), and Stage's about screen lists it wherever the voice ships.

## VocalSet: A Singing Voice Dataset

- Authors: Julia Wilkins, Prem Seetharaman, Alison Wahl, Bryan Pardo (Northwestern University).
- Citation: J. Wilkins, P. Seetharaman, A. Wahl, B. Pardo. "VocalSet: A Singing Voice Dataset."
  Proceedings of the 19th International Society for Music Information Retrieval Conference (ISMIR), 2018.
- Link: https://zenodo.org/records/1442513 (version 1.2, `VocalSet1-2.zip`).
- Licence: Creative Commons Attribution 4.0 International (CC BY 4.0).
  https://creativecommons.org/licenses/by/4.0/
- Modified: resampled to 48 kHz, peak-normalised, cut into training crops, and annotated with our own
  f0 curves, notes and vowel alignments. Used to train the Cantor singing voice (vocoder and acoustic
  model).

## LibriTTS

- Authors: Heiga Zen, Viet Dang, Rob Clark, Yu Zhang, Ron J. Weiss, Ye Jia, Zhifeng Chen, Yonghui Wu.
- Citation: H. Zen et al. "LibriTTS: A Corpus Derived from LibriSpeech for Text-to-Speech."
  Proc. Interspeech 2019.
- Link: https://www.openslr.org/60/ (subsets train-clean-100, train-clean-360, dev-clean, test-clean).
- Licence: Creative Commons Attribution 4.0 International (CC BY 4.0).
  https://creativecommons.org/licenses/by/4.0/
- Source: derived from LibriSpeech (V. Panayotov, G. Chen, D. Povey, S. Khudanpur, ICASSP 2015), which
  is built from public-domain LibriVox audiobook recordings.
- Modified: resampled to 48 kHz, peak-normalised, and annotated with our own phoneme transcriptions,
  f0 curves and alignments. Used to pre-train the Cantor singing voice (vocoder and acoustic model).

## Pronunciations

Lyrics and transcripts are phonemised with the English pronunciation lexicon embedded in
`libs/ai/models/speech` (Apache-2.0; see that crate's notices) and our own letter-to-sound rules.

## Download record

Downloaded 2026-09-30 on the training node, from the official sources above (licence text read on each
source page at download time):

| File | Bytes | SHA-256 |
|---|---|---|
| VocalSet1-2.zip | 5991573193 | 1745257239c9ff92cbd4ebb7ca52809f99682876b57495f01a7f69a1704f9b13 |
| LibriTTS dev-clean.tar.gz | 1291469655 | da0864e1bd26debed35da8a869dd5c04dfc27682921936de7cff9c8a254dbe1a |
| LibriTTS test-clean.tar.gz | 1230670113 | 234ea5b25859102a87024a4b9b86641f5b5aaaf1197335c95090cde04fe9a4f5 |
| LibriTTS train-clean-100.tar.gz | 7723686890 | c5608bf1ef74bb621935382b8399c5cdd51cd3ee47cec51f00f885a64c6c7f6b |
| LibriTTS train-clean-360.tar.gz | 27504073644 | ce7cff44dcac46009d18379f37ef36551123a1dc4e5c8e4eb73ae57260de4886 |
