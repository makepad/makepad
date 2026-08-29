//! Vendored synonym table for query expansion - GENERATED, never hand-edited.
//!
//! Source:    Princeton WordNet 3.1 database files (`data.noun`, `data.adj`,
//!            `data.verb`) from
//!            <https://wordnetcode.princeton.edu/wn3.1.dict.tar.gz>
//! License:   WordNet 3.1, Copyright 2011 by Princeton University, all rights
//!            reserved. "Permission to use, copy, modify and distribute this
//!            software and database and its documentation for any purpose and
//!            without fee or royalty is hereby granted, provided that you
//!            agree to comply with the following copyright notice and
//!            statements, including the disclaimer" - the full notice ships as
//!            `dict/LICENSE` in the tarball above and governs this derived
//!            extract, which reproduces synset membership only.
//! Extracted: 2026-08-28
//!
//! What is kept: the synsets of the concrete lexicographer files - every
//! adjective file, plus noun.Tops/animal/artifact/attribute/body/food/
//! location/object/person/phenomenon/plant/quantity/shape/substance and
//! verb.contact/verb.motion. An asset catalog describes things, not
//! abstractions, and the abstract files are where the polysemy lives.
//! Members are single lowercase ASCII-alphanumeric words of 2..=32 bytes -
//! exactly the tokens `search::tokenize_into` can produce, so every entry is
//! a term the index could actually hold. Dropped: groups left with fewer than
//! two members, groups over 12 members, and words appearing in more than four
//! groups (polysemy noise). Groups and members are sorted and deduplicated,
//! so regeneration is byte-exact.
//!
//! Regeneration (rewrites this file in place, header included):
//!
//! ```text
//! curl -sSL https://wordnetcode.princeton.edu/wn3.1.dict.tar.gz | tar xz -C /tmp
//! cd <repository root> && WN_DICT=/tmp/dict python3 - <<'PY'
//! import os, re
//! D = os.environ.get("WN_DICT", "/tmp/dict")
//! P = "libs/asset/store/src/synonyms/wordnet.rs"
//! M = "// ---- generated data: regenerate with the command above, never hand-edit ----\n"
//! KEEP = set("00 01 03 05 06 07 08 13 15 17 18 19 20 23 25 27 35 38 44".split())
//! syn = []
//! for f in ("noun", "adj", "verb"):
//!     for l in open(D + "/data." + f, encoding="latin-1"):
//!         if l.startswith("  "):
//!             continue
//!         p = l.split(" ")
//!         if len(p) < 5 or p[1] not in KEEP:
//!             continue
//!         try:
//!             n = int(p[3], 16)
//!         except ValueError:
//!             continue
//!         w = []
//!         for i in range(n):
//!             t = re.sub(r"\(.*\)$", "", p[4 + 2 * i]).lower()
//!             if re.fullmatch("[a-z0-9]{2,32}", t) and t not in w:
//!                 w.append(t)
//!         if 2 <= len(w) <= 12:
//!             syn.append(tuple(sorted(w)))
//! syn = sorted(set(syn))
//! seen = {}
//! for g in syn:
//!     for w in g:
//!         seen[w] = seen.get(w, 0) + 1
//! out = sorted({" ".join(w for w in g if seen[w] <= 4) for g in syn if sum(seen[w] <= 4 for w in g) >= 2})
//! idx, at = [], 0
//! for line in out:
//!     for w in line.split(" "):
//!         idx.append((w, at))
//!         at += len(w) + 1
//! idx.sort()
//! head = open(P, encoding="utf-8").read().split(M)[0]
//! b = [head, M, "\n/// Synonym groups: words joined by ' ', groups by '\\n'. Pure ASCII.\n"]
//! b.append('pub(super) static WORDNET_BLOB: &str = "\\\n')
//! b.append("".join(g + "\\n\\\n" for g in out))
//! b.append('";\n\n/// Byte offset of every word slot in the blob, sorted by the word it\n')
//! b.append("/// starts (ties by offset): the binary-search index.\n")
//! b.append("pub(super) static WORDNET_INDEX: &[u32] = &[\n")
//! for i in range(0, len(idx), 16):
//!     b.append("    " + " ".join(str(o) + "," for _, o in idx[i:i + 16]) + "\n")
//! b.append("];\n")
//! open(P, "w", encoding="utf-8").write("".join(b))
//! print("groups", len(out), "slots", len(idx), "unique", len(set(w for w, _ in idx)),
//!       "blob", sum(len(g) + 1 for g in out) - 1, "index", 4 * len(idx))
//! PY
//! ```

// ---- generated data: regenerate with the command above, never hand-edit ----

/// Synonym groups: words joined by ' ', groups by '\n'. Pure ASCII.
pub(super) static WORDNET_BLOB: &str = "\
10 decade ten tenner\n\
10 ten\n\
100 century hundred\n\
100 hundred\n\
1000 chiliad thou thousand yard\n\
1000 thousand\n\
10000 myriad\n\
100000 lakh\n\
1000000 meg million\n\
1000000000 billion\n\
1000000000000 billion\n\
1000000000000 trillion\n\
1000th thousandth\n\
100th centesimal hundredth\n\
101 ci\n\
105 cv\n\
10th tenth\n\
11 eleven xi\n\
110 cx\n\
115 cxv\n\
11th eleventh\n\
12 dozen twelve xii\n\
120 cxx\n\
125 cxxv\n\
12th twelfth\n\
13 thirteen xiii\n\
130 cxxx\n\
135 cxxxv\n\
13th thirteenth\n\
14 fourteen xiv\n\
140 cxl\n\
145 cxlv\n\
14th fourteenth\n\
15 fifteen xv\n\
150 cl\n\
155 clv\n\
15th fifteenth\n\
16 sixteen xvi\n\
160 clx\n\
165 clxv\n\
16th sixteenth\n\
17 seventeen xvii\n\
170 clxx\n\
175 clxxv\n\
17th seventeenth\n\
18 eighteen xviii\n\
180 clxxx\n\
18th eighteenth\n\
19 nineteen xix\n\
190 xcl\n\
19th nineteenth\n\
20 twenty xx\n\
200 cc\n\
20th twentieth\n\
21 xxi\n\
22 xxii\n\
23 xxiii\n\
24 xxiv\n\
25 xxv\n\
26 xxvi\n\
27 xxvii\n\
28 xxviii\n\
29 xxix\n\
2d 2nd second\n\
30 thirty xxx\n\
300 ccc\n\
30th thirtieth\n\
31 xxxi\n\
32 xxxii\n\
33 xxxiii\n\
34 xxxiv\n\
35 xxxv\n\
36 xxxvi\n\
37 xxxvii\n\
38 xxxviii\n\
39 ixl\n\
3rd tertiary third\n\
3tc lamivudine\n\
40 forty twoscore xl\n\
40 forty xl\n\
400 cd\n\
40th fortieth\n\
41 xli\n\
42 xlii\n\
43 xliii\n\
44 xliv\n\
45 xlv\n\
46 xlvi\n\
47 xlvii\n\
48 xlviii\n\
49 il\n\
4th fourth quaternary\n\
4to quarto\n\
50 fifty\n\
50th fiftieth\n\
51 li\n\
52 lii\n\
53 liii\n\
54 liv\n\
55 lv\n\
56 lvi\n\
57 lvii\n\
58 lviii\n\
59 ilx\n\
5th fifth\n\
60 lx sixty\n\
60 lx sixty threescore\n\
60th sixtieth\n\
61 lxi\n\
62 lxii\n\
63 lxiii\n\
64 lxiv\n\
65 lxv\n\
66 lxvi\n\
67 lxvii\n\
68 lxviii\n\
69 ilxx\n\
6th sixth\n\
70 lxx seventy\n\
70th seventieth\n\
71 lxxi\n\
72 lxxii\n\
73 lxxiii\n\
74 lxxiv\n\
75 lxxv\n\
76 lxxvi\n\
77 lxxvii\n\
78 lxxviii\n\
79 ilxxx\n\
7th seventh\n\
80 eighty fourscore lxxx\n\
80th eightieth\n\
81 lxxxi\n\
82 lxxxii\n\
83 lxxxiii\n\
84 lxxxiv\n\
85 lxxxv\n\
86 lxxxvi\n\
87 lxxxvii\n\
88 lxxxviii\n\
89 ixc\n\
8th eighth\n\
8vo eightvo octavo\n\
90 ninety xc\n\
90th ninetieth\n\
91 xci\n\
92 xcii\n\
93 xciii\n\
94 xciv\n\
95 xcv\n\
96 xcvi\n\
97 xcvii\n\
98 xcviii\n\
99 ic\n\
9th ninth\n\
aachen aken\n\
aalborg alborg\n\
aar aare\n\
aardvark anteater\n\
aarhus arhus\n\
ab abdominal\n\
abamp abampere\n\
abandon empty vacate\n\
abandon unconstraint wantonness\n\
abandoned derelict deserted\n\
abashed chagrined embarrassed\n\
abasic abatic\n\
abatis abattis\n\
abattoir butchery shambles slaughterhouse\n\
abaxial dorsal\n\
abbess prioress\n\
abbot archimandrite\n\
abbreviated brief\n\
abbreviated shortened truncated\n\
abbreviator abridger\n\
abdias obadiah\n\
abdomen belly stomach venter\n\
abdominous paunchy potbellied\n\
abdominousness paunchiness\n\
abducens abducent\n\
abducent abducting\n\
abduct kidnap nobble snatch\n\
abductor kidnaper kidnapper snatcher\n\
abenaki abnaki\n\
aberrant deviant deviate\n\
aberration distortion\n\
abetter abettor\n\
abeyant dormant\n\
abhorrent detestable obscene repugnant repulsive\n\
abiding enduring imperishable\n\
abila abyla\n\
abiogenesis autogenesis autogeny\n\
abject scummy scurvy\n\
abject unhopeful\n\
abkhas abkhasian abkhaz abkhazian\n\
abkhaz abkhazia\n\
abkhaz abkhazian\n\
ablaze afire aflame aflare alight\n\
ablaze aflame aroused\n\
ablaze inflamed reddened\n\
able capable\n\
abloom efflorescent\n\
ablutionary cleansing\n\
abnormal unnatural\n\
abnormality freakishness\n\
abode domicile dwelling habitation\n\
abode residence\n\
abolitionist emancipationist\n\
abominable atrocious dreadful painful terrible unspeakable\n\
abominable detestable execrable odious\n\
abominator loather\n\
aboriginal aborigine indigen indigene native\n\
aboriginal native\n\
aboriginal primaeval primal primeval primordial\n\
aborticide abortifacient\n\
abortive stillborn unsuccessful\n\
aboulic abulic\n\
abounding galore\n\
about astir\n\
aboveboard straightforward\n\
abradant abrader\n\
abradant abrasive\n\
abrade abrase corrade\n\
abrade scour\n\
abraham ibrahim\n\
abranchial abranchiate abranchious\n\
abrasion attrition detrition grinding\n\
abrasive harsh\n\
abrasive scratchy\n\
abrasiveness harshness scratchiness\n\
abroach broached\n\
abroad overseas\n\
abrupt disconnected\n\
abrupt precipitous\n\
abruptness brusqueness curtness gruffness shortness\n\
abruptness precipitance precipitancy precipitateness precipitousness suddenness\n\
abruptness precipitousness steepness\n\
abscond absquatulate bolt decamp\n\
abseil rappel\n\
abseiler rappeller\n\
absent absentminded abstracted scatty\n\
absent lacking missing wanting\n\
absinth absinthe\n\
absolute downright rank\n\
absolute infrangible inviolable\n\
absoluteness starkness utterness\n\
absolutist absolutistic\n\
absolved cleared exculpated exonerated vindicated\n\
absolvitory exonerative forgiving\n\
absorb imbibe suck\n\
absorbed captive engrossed enwrapped intent wrapped\n\
absorbefacient sorbefacient\n\
absorbent absorptive\n\
absorbing engrossing fascinating gripping riveting\n\
abstainer abstinent nondrinker\n\
abstainer ascetic\n\
abstention abstinence\n\
abstentious abstinent\n\
abstract abstractionist nonfigurative nonobjective\n\
abstracter abstractor\n\
abstruse recondite\n\
abstruseness obscureness obscurity reconditeness\n\
absurd cockeyed derisory idiotic laughable ludicrous nonsensical preposterous ridiculous\n\
absurdity fatuity fatuousness silliness\n\
abundance copiousness teemingness\n\
abused maltreated mistreated\n\
abuser maltreater\n\
abusive opprobrious scurrilous\n\
abut adjoin march\n\
abuzz buzzing\n\
abysm abyss\n\
abysmal abyssal unfathomable\n\
abyssinia ethiopia yaltopya\n\
ac actinium\n\
academic academician\n\
academic donnish pedantic\n\
academician schoolman\n\
academicism academism scholasticism\n\
acantha spine spur\n\
acanthisittidae xenicidae\n\
acanthoid acanthous spinous\n\
acapnial acapnic acapnotic\n\
acaracide acaricide\n\
acarpellous acarpelous\n\
acaryote akaryocyte akaryote\n\
acaudal acaudate\n\
acaulescent stemless\n\
accelerative acceleratory\n\
accelerator catalyst\n\
accelerator gas gun throttle\n\
accelerator throttle\n\
accented stressed\n\
acceptability acceptableness\n\
acceptable decent satisfactory\n\
acceptance sufferance toleration\n\
acceptant acceptive\n\
accepted recognised recognized\n\
access accession admission admittance entree\n\
access approach\n\
accessary accessory\n\
accessibility approachability\n\
accessibility availability availableness handiness\n\
accessible approachable\n\
accessory accouterment accoutrement\n\
accessory adjunct adjuvant ancillary appurtenant auxiliary\n\
accessory appurtenance supplement\n\
accho acre akka akko\n\
accidental inadvertent\n\
accidental incidental nonessential\n\
acclivitous rising uphill\n\
acclivity ascent climb upgrade\n\
accommodating accommodative\n\
accommodative cooperative\n\
accommodative reconciling\n\
accommodator obliger\n\
accompanied attended\n\
accompaniment complement\n\
accompanist accompanyist\n\
accompanying attendant collateral concomitant consequent ensuant incidental resultant sequent\n\
accomplice confederate\n\
accomplishable achievable doable manageable realizable\n\
accomplished complete\n\
accomplished completed realised realized\n\
accomplished effected established\n\
accordant agreeable concordant conformable consonant\n\
accoucheur obstetrician\n\
accoucheuse midwife\n\
accountability answerability answerableness\n\
accountant comptroller controller\n\
accoutered accoutred\n\
accredited commissioned licenced licensed\n\
accrued accumulated\n\
acculturational acculturative\n\
accumbent decumbent recumbent\n\
accumulative cumulative\n\
accumulator collector gatherer\n\
accuracy truth\n\
accurate exact precise\n\
accursed accurst maledict\n\
accusative accusatory accusing accusive\n\
accusative objective\n\
accustomed customary habitual wonted\n\
ace single unity\n\
ace super tiptop topnotch tops\n\
acebutolol sectral\n\
acellular noncellular\n\
acerate acerose acicular\n\
acerb acerbic acid acrid bitter blistering caustic sulfurous sulphurous virulent vitriolic\n\
acerb acerbic astringent\n\
acerbity acrimony bitterness jaundice tartness thorniness\n\
acerbity tartness\n\
acetabular cotyloid cotyloidal\n\
acetaldehyde ethanal\n\
acetamide ethanamide\n\
acetaminophen datril panadol phenaphen tempra tylenol\n\
acetanilid acetanilide phenylacetamide\n\
acetate ethanoate\n\
acetone propanone\n\
acetophenetidin acetphenetidin phenacetin\n\
acetose acetous vinegarish vinegary\n\
acetum vinegar\n\
acetylene alkyne ethyne\n\
achaean achaian\n\
acheronian acherontic stygian\n\
achiever succeeder success winner\n\
aching achy\n\
achira arrowroot\n\
achromasia lividity lividness luridness paleness pallidness pallor wanness\n\
achromatic neutral\n\
achromaticity achromatism colorlessness colourlessness\n\
achromic achromous\n\
achromycin tetracycline\n\
acid acidic acidulent acidulous\n\
acid dose dot elvis pane superman zen\n\
acidity sourness\n\
acidophil acidophile\n\
acidophilic acidophilous aciduric\n\
acinar acinic acinose acinous\n\
ackee akee\n\
acme apex vertex\n\
acned pimpled pimply pustulate\n\
acocanthera acokanthera\n\
acores azores\n\
acoustic acoustical\n\
acquaintance friend\n\
acquiescent biddable\n\
acragas agrigento\n\
acrid pungent\n\
acrididae locustidae\n\
acridity acridness\n\
acrilan polypropenonitrile\n\
acrimonious bitter\n\
acrobatic athletic gymnastic\n\
acrogenic acrogenous\n\
acrolein propenal\n\
acronymic acronymous\n\
acrylate propenoate\n\
acrylonitrile propenonitrile\n\
act deed\n\
acth adrenocorticotrophin adrenocorticotropin corticotrophin corticotropin\n\
actinaria actiniaria\n\
actinia actinian actiniarian\n\
actinide actinoid actinon\n\
actinometric actinometrical\n\
actinomorphic actinomorphous\n\
actinomycetal actinomycetous\n\
actinozoa anthozoa\n\
actinozoan anthozoan\n\
activated excited\n\
activating actuating\n\
active alive\n\
active dynamic\n\
active fighting\n\
active participating\n\
activeness activity\n\
activewear sportswear\n\
activist activistic\n\
activist militant\n\
actor doer worker\n\
actor histrion player thespian\n\
actual existent\n\
actual factual\n\
actual genuine literal\n\
actuary statistician\n\
acuate acute needlelike\n\
acular toradol\n\
aculeate aculeated\n\
acute discriminating incisive keen knifelike penetrating penetrative piercing\n\
acute intense\n\
acyclovir zovirax\n\
acylglycerol glyceride\n\
adalia antalya\n\
adam cristal ecstasy xtc\n\
adamance obduracy unyieldingness\n\
adamant adamantine inexorable intransigent\n\
adamant diamond\n\
adana seyhan\n\
adapin doxepin sinequan\n\
adaptative adaptive\n\
adapted altered\n\
adapter adaptor\n\
adapter arranger transcriber\n\
adaxial ventral\n\
addable addible\n\
addict freak junkie junky nut\n\
addition gain increase\n\
addition improver\n\
additive linear\n\
addlebrained addlepated muddleheaded puddingheaded\n\
addled befuddled muddled muzzy woolly wooly\n\
addlehead birdbrain loon\n\
adducent adducting adductive\n\
adenoidal nasal pinched\n\
adept expert practiced proficient skilful skillful\n\
adequacy adequateness\n\
adequacy sufficiency\n\
adequate enough\n\
adequate equal\n\
adequate passable tolerable\n\
adermin pyridoxal pyridoxamine pyridoxine\n\
adh pitressin vasopressin\n\
adhere bind bond\n\
adhere cleave cling cohere\n\
adherence adhesion adhesiveness bond\n\
adherent disciple\n\
adiposeness adiposity fattiness\n\
adiposis corpulence overweight stoutness\n\
adjacency contiguity contiguousness\n\
adjacent conterminous contiguous neighboring\n\
adjacent next\n\
adjectival adjective\n\
adjective procedural\n\
adjoin contact meet touch\n\
adjudicative adjudicatory\n\
adjunct assistant\n\
adjusted familiarised familiarized\n\
adjuster adjustor\n\
adjutant aide\n\
adman advertiser advertizer\n\
administrator executive\n\
admirability admirableness wonderfulness\n\
admirer adorer\n\
admirer booster champion friend protagonist\n\
admittable admittible\n\
admixture intermixture\n\
admonisher monitor reminder\n\
admonishing admonitory reproachful reproving\n\
admonitory cautionary exemplary monitory warning\n\
adnexa annexa\n\
adnexal annexal\n\
adolescent jejune juvenile puerile\n\
adolescent stripling teen teenager\n\
adolescent teen teenage teenaged\n\
adopted adoptive\n\
adorability adorableness\n\
adorable endearing lovely\n\
adored idolised idolized worshipped\n\
adoring doting fond\n\
adoring worshipful\n\
adorned decorated\n\
adpressed appressed\n\
adrenalin adrenaline epinephrin epinephrine\n\
adrenergic sympathomimetic\n\
adrenocorticotrophic adrenocorticotropic\n\
adrian hadrian\n\
adrianople adrianopolis edirne\n\
adrift afloat aimless directionless planless rudderless undirected\n\
adscript adscripted\n\
adsorbable adsorbate\n\
adsorbent adsorptive\n\
adulator flatterer\n\
adult grown grownup\n\
adult grownup\n\
adult pornographic\n\
adulterant adulterating\n\
adulterant adulterator\n\
adulterate adulterated debased\n\
adulterer fornicator\n\
adulteress fornicatress hussy jade slut strumpet trollop\n\
adulterous cheating\n\
adulterous extracurricular extramarital\n\
adumbrative foreshadowing prefigurative\n\
adust baked parched scorched sunbaked\n\
advance advanced\n\
advance beforehand\n\
advance progress\n\
advanced innovative modern\n\
advanced ripe\n\
advanced sophisticated\n\
advancing forward\n\
advantage reward\n\
advantage vantage\n\
advantageous favorable favourable\n\
advantageousness favorableness favourableness positiveness positivity profitableness\n\
adventitia tunic tunica\n\
adventurer explorer\n\
adventurer venturer\n\
adventuresome adventurous\n\
adventurousness venturesomeness\n\
adversary antagonist opponent opposer resister\n\
adversative oppositive\n\
adverse contrary\n\
adverse inauspicious untoward\n\
advertent heedful\n\
advil ibuprofen motrin nuprin\n\
adviser advisor consultant\n\
advisory consultative consultatory consultive\n\
advocate advocator exponent proponent\n\
advocate counsel counsellor counselor pleader\n\
adynamic asthenic debilitated enervated\n\
adynamic undynamic\n\
adz adze\n\
adzhar adzharia\n\
aegina aigina\n\
aegis breastplate egis\n\
aegospotami aegospotamos\n\
aengus angus oengus\n\
aeolia aeolis\n\
aeolian eolian\n\
aeolotropic eolotropic\n\
aeon eon\n\
aeonian ageless eonian eternal everlasting perpetual unceasing unending\n\
aeonian eonian\n\
aerated charged\n\
aerial aeriform aery airy ethereal\n\
aerial antenna\n\
aerie aery eyrie eyry\n\
aeriform airlike\n\
aerobic aerophilic aerophilous\n\
aerodrome airdrome airport drome\n\
aerodynamic flowing sleek streamlined\n\
aerofoil airfoil surface\n\
aerogenerator windmill\n\
aeronaut airman aviator flier flyer\n\
aeronautic aeronautical\n\
aerophyte epiphyte\n\
aeroplane airplane\n\
aerosolise aerosolize\n\
aerosolised aerosolized\n\
aesculapian medical\n\
aesculapius asclepius asklepios\n\
aesthete esthete\n\
aesthetic aesthetical esthetic esthetical\n\
aesthetic artistic esthetic\n\
aesthetic esthetic\n\
aesthetician esthetician\n\
aestival estival\n\
aetiologic aetiological etiologic etiological\n\
aetiologist etiologist\n\
afeard afeared\n\
affability affableness amiability amiableness bonhomie geniality\n\
affable amiable cordial genial\n\
affected moved stirred touched\n\
affected unnatural\n\
affecting poignant touching\n\
affectional affective emotive\n\
affectionate fond lovesome\n\
affectionateness fondness lovingness warmth\n\
affiliated attached connected\n\
affinal affine\n\
affirmable assertable\n\
affirmative affirmatory\n\
affirmative approbative approbatory approving plausive\n\
affirmative optimistic\n\
affirmer asserter asseverator avower declarer\n\
affix append supplement\n\
affixal affixial\n\
afflicted impaired\n\
afflicted stricken\n\
afflictive painful sore\n\
affluent confluent feeder tributary\n\
affluent flush loaded moneyed wealthy\n\
afforest forest\n\
afghan afghani afghanistani\n\
afghan afghanistani\n\
aflare flaring\n\
aflaxen aleve anaprox\n\
aflicker flickering\n\
afloat awash flooded inundated overflowing\n\
aflutter nervous\n\
afoot underway\n\
aforementioned aforesaid said\n\
aforethought planned plotted\n\
afoul fouled\n\
afrikaans afrikaner\n\
afrikander afrikaner boer\n\
aftermath backwash wake\n\
afters dessert\n\
agamic agamogenetic agamous apomictic parthenogenetic\n\
agape gaping\n\
agaze staring\n\
aged cured\n\
aged elderly older senior\n\
aged ripened\n\
agedness senescence\n\
ageing aging senescent\n\
agent broker factor\n\
ageratum mistflower\n\
aggeus haggai\n\
agglomerate agglomerated agglomerative clustered\n\
agglutinate agglutinative\n\
agglutinative polysynthetic\n\
aggravated provoked\n\
aggravating exacerbating exasperating\n\
aggravator annoyance\n\
aggregate aggregated aggregative mass\n\
aggregate combine\n\
aggregate sum total totality\n\
aggregator collector\n\
aggressive belligerent\n\
aggressiveness belligerence pugnacity\n\
aggressor assailant assaulter attacker\n\
aghast appalled dismayed shocked\n\
agile nimble\n\
agile nimble spry\n\
agility legerity lightness lightsomeness nimbleness\n\
agitate budge stir\n\
agitate commove disturb vex\n\
agitating agitative provoking\n\
agitator fomenter\n\
agkistrodon ancistrodon\n\
agleam gleaming nitid\n\
aglet aiglet\n\
aglet aiglet aiguilette\n\
aglitter coruscant fulgid glinting glistering glittering glittery scintillant scintillating sparkly\n\
aglow lambent lucent luminous\n\
agnail hangnail\n\
agnate agnatic paternal\n\
agnate patrikin patrisib\n\
agnostic agnostical\n\
agnostic doubter\n\
ago agone\n\
agonised agonized\n\
agonising agonizing excruciating harrowing torturesome torturing torturous\n\
agonist protagonist\n\
agonistic agonistical combative\n\
agonistic strained\n\
agrarian agricultural farming\n\
agreeability agreeableness\n\
agreeableness amenity\n\
agreement correspondence\n\
agrestic rustic\n\
agriculturalist agriculturist cultivator grower raiser\n\
agrimonia agrimony\n\
agrobiologic agrobiological\n\
agrologic agrological\n\
agronomic agronomical\n\
aguacate avocado\n\
agueweed boneset thoroughwort\n\
ahead leading\n\
ahorse ahorseback\n\
aid assistance help\n\
aide auxiliary\n\
aided assisted\n\
aides aidoneus hades\n\
aigret aigrette\n\
ail garlic\n\
ailing indisposed peaked poorly seedy sickly unwell\n\
aim bearing heading\n\
aimless drifting floating vagabond vagrant\n\
aimlessness purposelessness\n\
ain own\n\
air atmosphere\n\
air atmosphere aura\n\
air breeze zephyr\n\
aircraftman aircraftsman\n\
airdock hangar\n\
aired airy\n\
airfield field\n\
airheaded dizzy featherbrained giddy lightheaded silly\n\
airiness buoyancy\n\
airless stuffy unaired\n\
airline airway\n\
airs pose\n\
airscrew prop\n\
airship dirigible\n\
airsick carsick seasick\n\
airstream backwash race slipstream wash\n\
airstrip strip\n\
airt redirect\n\
airway skyway\n\
airwoman aviatress aviatrix\n\
airy impractical laputan visionary windy\n\
aisle gangway\n\
aizoaceae tetragoniaceae\n\
ak alaska\n\
akaba aqaba\n\
akhenaten akhenaton ikhanaton\n\
akin cognate consanguine consanguineal consanguineous kin\n\
akin kindred\n\
akmola astana\n\
akvavit aquavit\n\
al alabama\n\
al aluminium aluminum\n\
alabaman alabamian\n\
alabaster alabastrine\n\
alacrity briskness smartness\n\
alar alary aliform\n\
alar axillary\n\
alar daminozide\n\
alate alated\n\
albatross mollymawk\n\
albinal albinic albinistic albinotic\n\
albizia albizzia\n\
albumen albumin\n\
albumen ovalbumin\n\
albuminoid scleroprotein\n\
albuterol proventil ventolin\n\
alcahest alkahest\n\
alcalescent alkalescent\n\
alcapton alkapton\n\
alchemic alchemical\n\
alchemistic alchemistical\n\
alcides heracles herakles hercules\n\
alcohol inebriant intoxicant\n\
alcoholic alky boozer dipsomaniac soaker souse\n\
alcove bay\n\
alcyone halcyon\n\
aldactone spironolactone\n\
aldermanic aldermanly\n\
aldomet methyldopa\n\
alecost costmary\n\
alendronate fosamax\n\
alep aleppo halab\n\
alert alive awake\n\
alert brisk lively merry rattling spanking zippy\n\
alert watchful\n\
aleut aleutian\n\
alexander alexanders\n\
alfalfa lucerne\n\
alfilaria alfileria clocks filaree filaria\n\
alga algae\n\
algarobilla algarroba algarrobilla\n\
algarroba carob\n\
algebraic algebraical\n\
algeria algerie\n\
algometric algometrical\n\
algonkian algonkin\n\
algonkian algonquian algonquin\n\
algonquian algonquin\n\
alhacen alhazen\n\
alidad alidade\n\
alienage alienism\n\
alienated anomic disoriented\n\
alienated estranged\n\
alienee grantee\n\
alight perch\n\
aligning positioning\n\
alike like similar\n\
alikeness likeness similitude\n\
aliment alimentation nourishment nutriment nutrition sustenance victuals\n\
alimental alimentary nourishing nutrient nutritious nutritive\n\
alismales naiadales\n\
alive animated\n\
alive live\n\
aliveness animateness liveness\n\
alizarin alizarine\n\
alkalic alkaline\n\
alkaliser alkalizer antacid antiacid\n\
alkane paraffin\n\
alkanet bugloss\n\
alkene olefin olefine\n\
alkeran melphalan\n\
allayer comforter reliever\n\
alleged supposed\n\
allegiance fealty\n\
allegoric allegorical\n\
allegoriser allegorizer\n\
allele allelomorph\n\
allelic allelomorphic\n\
allen gracie\n\
allergic hypersensitised hypersensitive hypersensitized sensitised sensitized supersensitised supersensitive supersensitized\n\
alleviant alleviator palliative\n\
alleviated eased relieved\n\
alleviative alleviatory lenitive mitigative mitigatory palliative\n\
alley alleyway\n\
allice allis\n\
allied confederate confederative\n\
alligator gator\n\
alligatored cracked\n\
allioniaceae nyctaginaceae\n\
allmouth angler anglerfish goosefish lotte monkfish\n\
alloantibody isoantibody\n\
allocable allocatable apportionable\n\
allocator distributor\n\
allograft homograft\n\
allopurinol zyloprim\n\
allosaur allosaurus\n\
allotropic allotropical\n\
allotropism allotropy\n\
allowable permissible\n\
allowance leeway margin tolerance\n\
alloy metal\n\
allure allurement temptingness\n\
alluring beguiling enticing tempting\n\
alluviation sedimentation\n\
alluvion alluvium\n\
alluvion deluge flood inundation\n\
ally friend\n\
almandine almandite\n\
almighty creator godhead jehovah lord maker\n\
almighty omnipotent\n\
alone lone lonely\n\
alone only\n\
alone unequaled unequalled unique unparalleled\n\
aloneness loneliness lonesomeness solitariness\n\
aloof distant upstage\n\
aloofness remoteness standoffishness withdrawnness\n\
alpestrine subalpine\n\
alphabetic alphabetical\n\
alphabetised alphabetized\n\
alphabetiser alphabetizer\n\
alphameric alphamerical alphanumeric alphanumerical\n\
alprazolam xanax\n\
alsace alsatia elsass\n\
altace ramipril\n\
altarpiece reredos\n\
alterative curative healing remedial sanative therapeutic\n\
altered neutered\n\
alternate alternating\n\
alternate alternative\n\
alternate replacement surrogate\n\
althaea althea hollyhock\n\
altitude height\n\
alto contralto\n\
alto countertenor\n\
altruism selflessness\n\
altruist philanthropist\n\
altruistic selfless\n\
alula calypter\n\
alum alumna alumnus grad graduate\n\
alumbloom alumroot\n\
aluminise aluminize\n\
alupent metaproterenol\n\
alveolate cavitied faveolate honeycombed pitted\n\
alyssum madwort\n\
am americium\n\
amadavat avadavat\n\
amah housemaid maid maidservant\n\
amah wetnurse\n\
amalgamate amalgamated coalesced consolidated fused\n\
amalgamate commix mingle mix unify\n\
amanuensis stenographer\n\
amaranthine unfading\n\
amateur amateurish inexpert unskilled\n\
amateur recreational unpaid\n\
amative amorous\n\
amatory amorous romantic\n\
amazed astonied astonished astounded stunned\n\
amazing astonishing\n\
amazing awesome awing\n\
amazon virago\n\
ambagious circumlocutious circumlocutory periphrastic\n\
ambassador embassador\n\
amber gold\n\
amberfish amberjack\n\
ambiance ambience\n\
ambidexterity ambidextrousness\n\
ambidextrous deceitful duplicitous\n\
ambiguity equivocalness\n\
ambiguous equivocal\n\
ambit compass orbit scope\n\
ambition ambitiousness\n\
ambitionless unambitious\n\
ambitious challenging\n\
amble mosey\n\
ambler saunterer stroller\n\
ambo dais podium pulpit rostrum soapbox stump\n\
amboyna padauk padouk\n\
ambrosia beebread\n\
ambrosia bitterweed ragweed\n\
ambrosia nectar\n\
ambrosial ambrosian\n\
ambrosial ambrosian nectarous\n\
ambulant ambulatory\n\
ameba amoeba\n\
ameban amebic amebous amoeban amoebic amoebous\n\
ameboid amoeboid\n\
ameer amir emeer emir\n\
ameliorating ameliorative amelioratory meliorative\n\
amen amon amun\n\
amenability amenableness cooperativeness\n\
amenable conformable\n\
amenable tractable\n\
amendable correctable\n\
amenorrheal amenorrheic amenorrhoeal amenorrhoeic\n\
ament catkin\n\
amentaceous amentiferous\n\
america us usa\n\
amerind amerindic indian\n\
ametabolic ametabolous\n\
amethopterin methotrexate\n\
amex curb\n\
amicability amicableness\n\
amidopyrine aminopyrine\n\
amine aminoalkane\n\
aminic amino\n\
aminobenzine aniline phenylamine\n\
aminopherase aminotransferase transaminase\n\
amiodarone cordarone\n\
amiss awry haywire\n\
amitriptyline elavil\n\
amity cordiality\n\
ammo ammunition\n\
ammoniac ammoniacal\n\
ammonite ammonoid\n\
amnesiac amnesic\n\
amnesic amnestic\n\
amnic amnionic amniotic\n\
amnion amnios\n\
amoebida amoebina\n\
amok amuck berserk\n\
amor cupid\n\
amorphous formless shapeless\n\
amorphous uncrystallised uncrystallized\n\
amorphous unstructured\n\
amount measure quantity\n\
amoxicillin amoxil augmentin larotid polymox trimox\n\
amp ampere\n\
amphetamine speed upper\n\
amphibian amphibious\n\
amphibiotic semiaquatic\n\
amphicarpa amphicarpaea\n\
amphioxidae branchiostomidae\n\
amphioxus lancelet\n\
amphiprostylar amphiprostyle amphistylar porticoed\n\
amphiprotic amphoteric\n\
amphisbaena amphisbaenia\n\
amphitheater amphitheatre\n\
amphitheater amphitheatre coliseum\n\
amphitheatric amphitheatrical\n\
ampicillin polycillin principen\n\
ample copious plenteous plentiful rich\n\
ample sizable sizeable\n\
amplification gain\n\
amplitude bountifulness bounty\n\
ampoule ampul ampule phial vial\n\
ampullar ampullary\n\
amrinone inocor\n\
amulet talisman\n\
amur heilong\n\
amused diverted entertained\n\
amusing amusive diverting\n\
amusing comic comical funny laughable mirthful risible\n\
amygdaliform amygdaloid amygdaloidal\n\
amylaceous amyloid amyloidal farinaceous starchlike\n\
amylum starch\n\
anachronic anachronistic anachronous\n\
anaemic anemic\n\
anaerobic anaerobiotic\n\
anaesthetic anesthetic\n\
anaesthetist anesthesiologist anesthetist\n\
anaglyphic anaglyphical anaglyptic anaglyptical\n\
anagogic anagogical\n\
anagrammatic anagrammatical\n\
analgesic analgetic anodyne\n\
analgesic anodyne painkiller\n\
analog analogue linear\n\
analog analogue parallel\n\
analogous correspondent\n\
analphabet analphabetic\n\
analphabetic unlettered\n\
analyser analyzer\n\
analyst psychoanalyst\n\
analytic analytical\n\
analytic uninflected\n\
analyzable decomposable\n\
anamorphism anamorphosis\n\
ananas pineapple\n\
anapaestic anapestic\n\
anapurna annapurna\n\
anapurna annapurna parvati\n\
anarchic anarchical lawless\n\
anarchist nihilist syndicalist\n\
anastigmatic stigmatic\n\
anastomose inosculate\n\
anastomosis inosculation\n\
anatomic anatomical\n\
anatomise anatomize\n\
anatomy bod build chassis flesh physique shape soma\n\
anatoxin toxoid\n\
anatropous inverted\n\
ancestor antecedent ascendant ascendent root\n\
ancestral hereditary patrimonial transmissible\n\
ancestry derivation filiation lineage\n\
anchor anchorman anchorperson\n\
anchorite hermit\n\
anchoritic eremitic eremitical hermitic hermitical\n\
ancient antediluvian\n\
ancientness antiquity\n\
andalucia andalusia\n\
andelmin angelim\n\
andiron firedog\n\
andrena andrenid\n\
androgenetic androgenous\n\
androgyne epicene gynandromorph hermaphrodite intersex\n\
androgyny bisexuality hermaphroditism\n\
android humanoid\n\
anecdotal anecdotic anecdotical\n\
anecdotist raconteur\n\
anemometric anemometrical\n\
anemone windflower\n\
anencephalic anencephalous\n\
anestric anestrous anoestrous\n\
aneurin thiamin thiamine\n\
aneurismal aneurismatic aneurysmal aneurysmatic\n\
angara tunguska\n\
angel backer\n\
angel saint\n\
angelfish monkfish\n\
angelfish spadefish\n\
angelic angelical\n\
angelic angelical beatific sainted saintlike saintly\n\
angelic angelical cherubic seraphic\n\
angelica angelique\n\
angered enraged furious infuriated maddened\n\
angevin angevine\n\
anginal anginose anginous\n\
angiocarpic angiocarpous\n\
angiospermae anthophyta magnoliophyta\n\
angiotensin angiotonin hypertensin\n\
angle fish\n\
angle slant tilt\n\
angler troller\n\
anglesea anglesey mona\n\
angleworm crawler earthworm fishworm nightcrawler nightwalker wiggler\n\
anglophil anglophile\n\
angora ankara\n\
angry furious raging tempestuous\n\
anguillula turbatrix\n\
anguished tormented tortured\n\
angular angulate\n\
anhinga darter snakebird\n\
anil indigo indigotin\n\
animal beast brute creature fauna\n\
animal carnal fleshly sensual\n\
animalcule animalculum\n\
animalism physicality\n\
animate sentient\n\
animating enlivening\n\
animation brio invigoration spiritedness vivification\n\
animation vitality\n\
animator energiser energizer vitaliser vitalizer\n\
animist animistic\n\
anise aniseed\n\
anisogamic anisogamous\n\
anisometric unsymmetrical\n\
anklebone astragal astragalus talus\n\
anklet anklets bobbysock bobbysocks\n\
ankylosaur ankylosaurus\n\
anlage primordium\n\
annam vietnam\n\
annamese vietnamese\n\
annelid annelidan\n\
annex annexe extension wing\n\
annihilated exterminated\n\
annihilating annihilative devastating withering\n\
annihilating devastating withering\n\
annon sweetsop\n\
announced proclaimed\n\
annoyed harassed harried pestered vexed\n\
annoyed irritated miffed nettled peeved pissed riled roiled steamed stung\n\
annoyer tease teaser vexer\n\
annual yearly\n\
annular annulate annulated circinate ringed\n\
annulet bandelet bandelette bandlet\n\
annulus doughnut halo ring\n\
annulus skirt\n\
anodal anodic\n\
anomie anomy\n\
anorak parka windbreaker windcheater\n\
anorectic anorexic\n\
anorectic anorexigenic\n\
anorthic triclinic\n\
anosmatic anosmic\n\
anovulant pill\n\
anpu anubis\n\
ansaid flurbiprofen\n\
anserine dopey dopy foolish gooselike goosey goosy jerky\n\
answerer respondent responder\n\
answering respondent\n\
ant emmet pismire\n\
antabuse disulfiram\n\
antagonistic antipathetic antipathetical\n\
antagonistic counter\n\
antakiya antakya antioch\n\
anteater echidna\n\
anteater numbat\n\
anteater pangolin\n\
antecedence antecedency anteriority precedence precedency priority\n\
antechamber anteroom foyer hall lobby vestibule\n\
antediluvial antediluvian\n\
antediluvian antiquated archaic\n\
antenatal antepartum prenatal\n\
antenna feeler\n\
antennal antennary\n\
antenuptial premarital prenuptial\n\
anterior prior\n\
anthelminthic anthelmintic helminthic parasiticidal\n\
anthelminthic anthelmintic helminthic vermifuge\n\
antheral staminate\n\
antherozoid spermatozoid\n\
anthill formicary\n\
anthony antonius antony\n\
anthophagous anthophilous\n\
anthropic anthropical\n\
anthropogenetic anthropogenic\n\
anthropoid anthropoidal apelike\n\
anthropoid ape\n\
anthropoid manlike\n\
anthropometric anthropometrical\n\
anthropomorphic anthropomorphous humanlike\n\
anthropophagite anthropophagus cannibal\n\
anthurium tailflower\n\
antiaircraft flack flak\n\
antibacterial bactericide\n\
antiblack racist\n\
antic fantastic fantastical grotesque\n\
anticancer antineoplastic antitumor antitumour\n\
anticipant anticipative expectant\n\
anticipant anticipator\n\
anticipated awaited\n\
anticipatory prevenient\n\
anticlimactic anticlimactical\n\
anticlockwise contraclockwise counterclockwise\n\
anticoagulant decoagulant\n\
anticonvulsant antiepileptic\n\
antidote counterpoison\n\
antielectron positron\n\
antifertility contraceptive prophylactic\n\
antifungal antimycotic fungicide\n\
antifungal fungicidal\n\
antigonus monophthalmos\n\
antiknock antiknocking\n\
antimicrobial antimicrobic\n\
antimicrobial antimicrobic disinfectant germicide\n\
antimonic antimonious\n\
antimonopoly antitrust\n\
antimony sb\n\
antipathetic antipathetical averse indisposed loath loth\n\
antiphonal antiphonary\n\
antiphonal responsive\n\
antipodal antipodean\n\
antipsychotic neuroleptic\n\
antipyretic febrifuge\n\
antiquarian antiquary archaist\n\
antique demode ex outmoded passe passee\n\
antique gaffer oldtimer\n\
antisatellite asat\n\
antisocial asocial\n\
antispasmodic spasmolytic\n\
antithetic antithetical\n\
antitypic antitypical\n\
antivenene antivenin\n\
antivert meclizine\n\
antlion doodlebug\n\
antoninus aurelius\n\
antsy fidgety fretful itchy\n\
antwerp antwerpen anvers\n\
anunnaki enuki\n\
anura batrachia salientia\n\
anuran batrachian frog salientian toad\n\
anuran batrachian salientian\n\
anuretic anuric\n\
anurous tailless\n\
anvil incus\n\
anxious dying\n\
anxious nervous queasy uneasy unquiet\n\
any whatever whatsoever\n\
aortal aortic\n\
aoudad arui audad\n\
apathy indifference numbness spiritlessness\n\
apatosaur apatosaurus brontosaur brontosaurus\n\
ape aper copycat emulator imitator\n\
apelike apish\n\
aperient cathartic physic purgative\n\
aperiodic nonperiodic\n\
apetalous petalless\n\
aphaeretic apheretic\n\
aphonic voiceless\n\
aphoristic apothegmatic epigrammatic\n\
aphoristic axiomatic\n\
aphrodisiac aphrodisiacal sexy\n\
aphrodite cytherea\n\
apiaceae umbelliferae\n\
apiarist apiculturist beekeeper\n\
aplacophora solenogastres\n\
aplacophoran solenogaster\n\
aplomb assuredness cool poise\n\
aplysia tethys\n\
aplysiidae tethyidae\n\
apneic apnoeic\n\
apocalyptic apocalyptical revelatory\n\
apodal apodous\n\
apodeictic apodictic\n\
apogametic apogamic apogamous\n\
apolitical unpolitical\n\
apollo phoebus\n\
apologetic excusatory\n\
apologist justifier vindicator\n\
apolune aposelene\n\
apomictic apomictical\n\
apoplectiform apoplectoid\n\
apostate deserter ratter recreant renegade turncoat\n\
apostolic apostolical\n\
apostolic apostolical papal pontifical\n\
apothecary chemist druggist pharmacist\n\
apothegmatic apothegmatical\n\
apotheosis ideal nonesuch nonpareil nonsuch paragon saint\n\
appalling dismaying\n\
apparatus setup\n\
apparel clothes\n\
appareled attired dressed garbed garmented habilimented robed\n\
apparency apparentness\n\
apparent evident manifest palpable patent unmistakable\n\
apparent ostensible seeming\n\
apparition fantasm phantasm phantasma phantom specter spectre\n\
apparitional ghostlike ghostly phantasmal spectral spiritual\n\
appeal appealingness charm\n\
appealing likable likeable sympathetic\n\
appeasable conciliable\n\
appeasing placating placative placatory\n\
appellant appellate\n\
appellative naming\n\
append tack\n\
appendage extremity member\n\
appendage outgrowth process\n\
appetiser appetizer starter\n\
appetising appetizing\n\
appetisingness appetizingness\n\
applaudable commendable laudable praiseworthy\n\
applauder clapper\n\
appliance contraption contrivance convenience gadget gismo gizmo widget\n\
applicant applier\n\
application lotion\n\
applicative applicatory\n\
applicator applier\n\
appointed appointive\n\
appointed decreed ordained prescribed\n\
appointee appointment\n\
appointment fitting\n\
apposable opposable\n\
apposite apt pertinent\n\
appositeness aptness\n\
appositional appositive\n\
appraiser authenticator\n\
appraiser valuator\n\
appraising evaluative\n\
appreciated apprehended comprehended\n\
apprehend arrest collar cop nab nail\n\
apprehender knower\n\
apprehensible graspable intelligible perceivable understandable\n\
apprehensive discerning\n\
apprehensive worried\n\
apprentice learner prentice\n\
apprenticed articled indentured\n\
approach approaching coming\n\
approachable reachable\n\
approaching coming forthcoming upcoming\n\
appropriateness rightness\n\
approved sanctioned\n\
approximate approximative\n\
appurtenance gear paraphernalia\n\
apractic apraxic\n\
apresoline hydralazine\n\
apricot peach\n\
apron forestage proscenium\n\
apse apsis\n\
apt clever\n\
apt disposed given minded tending\n\
apt liable\n\
apteral apterous\n\
apteryx kiwi\n\
aptness propensity\n\
apulia puglia\n\
aqua aquamarine turquoise\n\
aquacultural aquicultural hydroponic\n\
aqualung scuba\n\
aquanaut oceanaut\n\
aquaphobic hydrophobic\n\
aqueous sedimentary\n\
aquilege aquilegia columbine\n\
aquiline hooked\n\
aquitaine aquitania\n\
ar are\n\
ar argon\n\
ar arkansas\n\
arab arabian\n\
arable cultivable cultivatable tillable\n\
araceous aroid\n\
arachnid arachnoid\n\
arachnidian arachnoid spiderlike spiderly spidery\n\
araguaia araguaya\n\
arak arrack\n\
aramaean aramean\n\
aranea araneus\n\
araneae araneida\n\
araneidal araneidan\n\
arapaho arapahoe\n\
araroba chrysarobin\n\
aras araxes\n\
arava leflunomide\n\
arawak arawakan\n\
arb arbitrager arbitrageur\n\
arbalest arbalist ballista bricole catapult mangonel onager trebuchet trebucket\n\
arbiter arbitrator umpire\n\
arbitral arbitrational\n\
arbitrariness capriciousness flightiness whimsey whimsicality whimsy\n\
arbor arbour bower pergola\n\
arbor mandrel mandril spindle\n\
arboraceous arboreous woodsy woody\n\
arborary arboreal arborical arborous\n\
arboreal arboreous\n\
arboreal arboreous arborescent arboresque arboriform dendriform dendroid dendroidal treelike\n\
arboriculturist forester\n\
arborvirus arbovirus\n\
arc arch\n\
arc bow\n\
arc discharge spark\n\
arcade colonnade\n\
arcadian bucolic pastoral\n\
arcdegree degree\n\
arced arched arching arciform arcuate bowed\n\
arch archway\n\
arch condescending patronising patronizing\n\
arch impish implike mischievous pixilated prankish puckish wicked\n\
archaean archean\n\
archaebacteria archaebacterium archaeobacteria archeobacteria\n\
archaeologic archaeological archeologic archeological\n\
archaeologist archeologist\n\
archaeopteryx archeopteryx\n\
archaeozoic archeozoic\n\
archaic primitive\n\
archangelic archangelical\n\
archegonial archegoniate\n\
archepiscopal archiepiscopal\n\
archer bowman\n\
archer sagittarius\n\
archespore archesporium\n\
archetypal archetypical prototypal prototypic prototypical\n\
archil cudbear orchil\n\
archil orchil\n\
archipallium paleocortex\n\
architect designer\n\
architectonic tectonic\n\
archness impertinence perkiness pertness sauciness\n\
archosaur archosaurian\n\
archpriest hierarch prelate primate\n\
arcminute minute\n\
arcsecond second\n\
arctic frigid gelid glacial icy polar\n\
arctic galosh golosh gumshoe rubber\n\
ardent fervent fervid fiery impassioned perfervid torrid\n\
ardent warm\n\
arduous backbreaking grueling gruelling laborious operose punishing toilsome\n\
arduous straining strenuous\n\
arduousness strenuousness\n\
area country\n\
area expanse\n\
area region\n\
arecaceae palmaceae palmae\n\
arena bowl stadium\n\
arenaceous sandlike sandy\n\
areolar areolate\n\
arere obeche obechi samba\n\
argal argali\n\
argent silverish silvery\n\
argentine argentinian\n\
argillaceous clayey\n\
argonaut nautilus\n\
arguable debatable disputable moot\n\
arguer debater\n\
argyle argyll\n\
arhant arhat lohan\n\
arianrhod arianrod\n\
aricara arikara\n\
arid desiccate desiccated\n\
arid waterless\n\
aridity barrenness fruitlessness\n\
ariled arillate\n\
ariose songlike\n\
arise uprise\n\
arishth margosa neem\n\
aristocort aristopak kenalog triamcinolone\n\
aristocrat patrician\n\
aristocratic aristocratical patrician\n\
aristotelean aristotelian aristotelic peripatetic\n\
aristotelean aristotelian peripatetic\n\
arithmetic arithmetical\n\
arizona az\n\
arizonan arizonian\n\
arkansan arkansawyer\n\
arm branch limb\n\
arm sleeve\n\
arm weapon\n\
armenia hayastan\n\
armguard bracer\n\
armin arminius hermann\n\
armor armour\n\
armored armoured\n\
armored panoplied\n\
armorer armourer\n\
armorer armourer artificer\n\
armory armoury arsenal\n\
armpit axilla\n\
arms blazon blazonry\n\
arms munition weaponry\n\
armstrong satchmo\n\
aroid arum\n\
aroma odor odour scent smell\n\
aromatic redolent\n\
arouet voltaire\n\
arouse stir\n\
aroused emotional excited\n\
aroused horny randy ruttish steamy\n\
aroused stimulated stirred\n\
arouser rouser waker\n\
arquebus hackbut hagbut harquebus\n\
arranged ordered\n\
arranged staged\n\
arrangement placement\n\
arranger organiser organizer\n\
arras tapestry\n\
array raiment regalia\n\
arrayed panoplied\n\
arrest halt\n\
arresting sensational stunning\n\
arrhythmic arrhythmical\n\
arrhythmic jerking jerky\n\
arrival arriver comer\n\
arrive come\n\
arriviste parvenu upstart\n\
arrogance haughtiness hauteur lordliness\n\
arrogant chesty\n\
arrowworm chaetognath\n\
arse arsehole asshole bunghole\n\
arsenic as\n\
arsenic ratsbane\n\
arsenopyrite mispickel\n\
arsonist firebug incendiary\n\
artefact artifact\n\
artefactual artifactual\n\
artemia chirocephalus\n\
artemis cynthia\n\
arteria artery\n\
arteriola arteriole\n\
artful disingenuous\n\
arthritic creaky rheumatic rheumatoid rheumy\n\
arthropodal arthropodan arthropodous\n\
arthrosporic arthrosporous\n\
articular articulary\n\
articulate articulated\n\
articulateness fluency volubility\n\
articulatio articulation joint\n\
articulation join joint junction juncture\n\
articulative articulatory\n\
artificer artisan craftsman journeyman\n\
artificer discoverer inventor\n\
artificial contrived hokey stilted\n\
artificial unreal\n\
artillery gun ordnance\n\
artilleryman cannoneer gunner\n\
artiodactyl artiodactylous\n\
artless ingenuous\n\
artless uncultivated uncultured\n\
artlessness ingenuousness innocence naturalness\n\
arugula rocket roquette\n\
arytaenoid arytenoid\n\
asafetida asafoetida\n\
ascend uprise\n\
ascendable ascendible climbable\n\
ascendant ascendent ascensive\n\
ascendant ascendent dominating\n\
ascertainable discoverable\n\
ascertained discovered observed\n\
ascetic ascetical\n\
ascetic ascetical austere spartan\n\
asceticism austerity nonindulgence\n\
aschelminthes nematoda\n\
ascomycota ascomycotina\n\
ascosporic ascosporous\n\
ascribable due imputable referable\n\
asdic sonar\n\
aseptic sterile\n\
asexual nonsexual\n\
asexuality sexlessness\n\
ashbin ashcan dustbin wastebin\n\
ashen blanched bloodless livid\n\
ashir ashur\n\
ashtoreth astarte\n\
ashur assur asur\n\
ashurbanipal assurbanipal asurbanipal\n\
asian asiatic\n\
asinine fatuous inane\n\
askance askant asquint sidelong squint squinty\n\
asker enquirer inquirer querier questioner\n\
askew awry cockeyed lopsided wonky\n\
aslant aslope diagonal slanted slanting sloped sloping\n\
asleep benumbed numb\n\
asleep deceased departed\n\
asmara asmera\n\
aspadana esfahan isfahan\n\
asparaginase elspar\n\
aspect expression face look\n\
aspergill aspersorium\n\
aspergillales eurotiales\n\
asperity grimness hardship rigor rigorousness rigour rigourousness severeness severity\n\
aspheric aspherical\n\
asphyxiate choke stifle suffocate\n\
asphyxiate smother suffocate\n\
asphyxiator extinguisher\n\
aspinwall colon\n\
aspirant aspirer hopeful wannabe wannabee\n\
aspirant aspiring wishful\n\
aspirin bayer empirin\n\
assagai assegai\n\
assailable undefendable undefended\n\
assassin assassinator bravo\n\
assaultive attacking\n\
assemblage hookup\n\
assembly forum\n\
asserting declarative declaratory\n\
asset plus\n\
asshole bastard cocksucker dickhead motherfucker shit sob whoreson\n\
assiduity assiduousness concentration\n\
assiduous sedulous\n\
assignable conveyable negotiable transferable transferrable\n\
assimilate imbibe\n\
assimilating assimilative assimilatory\n\
assimilator learner scholar\n\
assistant help helper\n\
associability associableness\n\
associate companion comrade familiar\n\
associative associatory\n\
assorted miscellaneous mixed motley sundry\n\
assorted various\n\
assouan assuan aswan\n\
assuasive soothing\n\
assume strike\n\
assumed fictitious fictive pretended sham\n\
assuming assumptive presumptuous\n\
assumption effrontery presumption presumptuousness\n\
astacidae astacura\n\
astatine at\n\
asteraceae compositae\n\
asterisked starred\n\
asterope sterope\n\
asthmatic wheezing wheezy\n\
astir up\n\
astomatous mouthless\n\
astonishing astounding staggering stupefying\n\
astounding dumbfounding dumfounding\n\
astragal bead beading beadwork\n\
astral stellar\n\
astrantia masterwort\n\
astringency stypsis\n\
astringent styptic\n\
astroglia macroglia\n\
astrologer astrologist\n\
astronaut cosmonaut spaceman\n\
astronautic astronautical\n\
astronomer stargazer uranologist\n\
astronomic astronomical\n\
astronomic astronomical galactic\n\
astute shrewd\n\
asylum institution\n\
asylum refuge sanctuary\n\
asymmetric asymmetrical\n\
asymmetrical crooked\n\
asymmetry dissymmetry imbalance\n\
asymptomatic symptomless\n\
atabrine mepacrine quinacrine\n\
atactic ataxic\n\
atakapa attacapan\n\
atar athar attar ottar\n\
ataractic ataraxic sedative tranquilising tranquilizing tranquillising tranquillizing\n\
ataractic tranquilizer tranquilliser tranquillizer\n\
atarax hydroxyzine vistaril\n\
atavist throwback\n\
atavistic throwback\n\
aten aton\n\
atenolol tenormin\n\
athabascan athabaskan athapascan athapaskan\n\
atheist atheistic atheistical\n\
atheistic atheistical unbelieving\n\
athena athene pallas\n\
athenaeum atheneum\n\
athens athinai\n\
atherodyde athodyd ramjet\n\
atheromatic atheromatous\n\
athirst hungry thirsty\n\
athlete jock\n\
athleticism strenuosity\n\
athyriaceae dryopteridaceae\n\
atilt canted leaning tilted tipped\n\
ativan lorazepam\n\
atlantides hesperides\n\
atlas telamon\n\
atm atmosphere\n\
atmometer evaporometer\n\
atmospheric atmospherical\n\
atom corpuscle molecule mote particle speck\n\
atomic nuclear\n\
atomise atomize\n\
atomiser atomizer nebuliser nebulizer spray sprayer\n\
atomistic atomistical\n\
atonal unkeyed\n\
atonic unaccented\n\
atorvastatin lipitor\n\
atoxic nontoxic\n\
atrabilious bilious dyspeptic liverish\n\
atrioventricular auriculoventricular\n\
atrip aweigh\n\
atrocious flagitious grievous monstrous\n\
atrocious frightful horrible horrifying ugly\n\
atrociousness atrocity barbarity barbarousness heinousness\n\
atrophied diminished wasted\n\
attached committed\n\
attachment bond\n\
attain gain hit\n\
attain hit\n\
attempter essayer trier\n\
attendant attendee attender meeter\n\
attendant attender\n\
attender auditor hearer listener\n\
attentive heedful thoughtful\n\
attenuate attenuated faded weakened\n\
attestant attestator attestor witness\n\
attestant attester\n\
attested authenticated documented\n\
attic bean bonce dome noggin noodle\n\
attic garret loft\n\
attire garb\n\
attitude position posture\n\
attorney lawyer\n\
attracter attraction attractor\n\
attraction attractiveness\n\
attributive prenominal\n\
atypic atypical untypical\n\
au gold\n\
auberge hostel hostelry inn lodge\n\
aubergine brinjal eggplant\n\
aubergine eggplant\n\
auctorial authorial\n\
audacious barefaced bodacious brassy brazen insolent\n\
audacious brave dauntless fearless hardy intrepid unfearing\n\
audacious daring venturesome venturous\n\
audaciousness audacity\n\
audaciousness audacity temerity\n\
audibility audibleness\n\
audible hearable\n\
audile auditive auditory\n\
audiometer sonometer\n\
auger gimlet wimble\n\
auger snake\n\
augmentative enhancive\n\
augur auspex\n\
august lordly\n\
august revered venerable\n\
augustus octavian\n\
aunt auntie aunty\n\
aura aureole gloriole glory halo nimbus\n\
aureate flamboyant florid\n\
aureate gilded gilt gold\n\
aureole corona\n\
aureomycin chlortetracycline\n\
auric aurous\n\
auricle ear pinna\n\
auricular otic\n\
auriculate auriculated\n\
auriga charioteer\n\
auriscope auroscope otoscope\n\
aurochs urus\n\
aurochs wisent\n\
auroral aurorean\n\
auspiciousness propitiousness\n\
aussie australian\n\
austere stark\n\
austereness severeness severity\n\
austria oesterreich\n\
autacoid autocoid\n\
autarchic autarchical autarkical\n\
autarkic autarkical\n\
authentic reliable\n\
authentic unquestionable veritable\n\
authenticity genuineness legitimacy\n\
author generator source\n\
author writer\n\
authorisation authority authorization dominance potency\n\
authorisation authority authorization sanction\n\
authorised authoritative authorized\n\
authorised authorized\n\
authoriser authorizer\n\
authoritarian autocratic despotic dictatorial tyrannic tyrannical\n\
authoritarian dictator\n\
authoritarian dictatorial overbearing\n\
authoritative definitive\n\
authoritative important\n\
auto automobile car machine motorcar\n\
autobiographic autobiographical\n\
autobus bus charabanc coach jitney motorbus motorcoach omnibus\n\
autochthonal autochthonic autochthonous endemic indigenous\n\
autochthony endemism indigenousness\n\
autoclave steriliser sterilizer\n\
autocrat despot tyrant\n\
autocratic bossy dominating magisterial peremptory\n\
autocue prompter\n\
autoecious homoecious\n\
autogamic autogamous\n\
autogenic autogenous\n\
autogiro autogyro gyroplane\n\
autograft autoplasty\n\
autoloading semiautomatic\n\
automatic automatonlike machinelike robotic robotlike\n\
automatic reflex reflexive\n\
automaton golem robot\n\
automaton zombi zombie\n\
automobilist motorist\n\
autonomous independent sovereign\n\
autophyte autotroph\n\
autophytic autotrophic\n\
autotomise autotomize\n\
autotype facsimile\n\
auxiliary subsidiary supplemental supplementary\n\
avail help service\n\
available uncommitted\n\
available usable useable\n\
avalokiteshvara avalokitesvara\n\
avarice avariciousness covetousness cupidity\n\
avaricious covetous grabby grasping greedy prehensile\n\
avatar embodiment incarnation\n\
avellan avellane\n\
avenger retaliator\n\
aventail camail ventail\n\
aventurine sunstone\n\
avenue boulevard\n\
averageness mediocrity\n\
avertable avertible avoidable evitable\n\
aviary volary\n\
aviate pilot\n\
avid devouring esurient greedy\n\
avid zealous\n\
avifaunal avifaunistic\n\
avoirdupois blubber fat fatness\n\
avowed professed\n\
aware cognisant cognizant\n\
aware mindful\n\
aweary weary\n\
awed awestricken awestruck\n\
aweless awless\n\
aweless awless disrespectful\n\
awfulness dreadfulness horridness terribleness\n\
awkward bunglesome clumsy ungainly\n\
awkward clumsy cumbersome inapt inept\n\
awkward embarrassing sticky unenviable\n\
awkward uneasy\n\
awkwardness clumsiness\n\
awkwardness clumsiness gracelessness stiffness\n\
awkwardness cumbersomeness unwieldiness\n\
awned awny\n\
awning sunblind sunshade\n\
awol truant\n\
ax axe\n\
axial axile\n\
axiomatic axiomatical postulational\n\
axon axone\n\
az azimuth\n\
azactam aztreonam\n\
azathioprine imuran\n\
azedarach azederach chinaberry\n\
azerbaijan azerbajdzhan\n\
azithromycin zithromax\n\
azoimide hn\n\
azotemic uraemic uremic\n\
azotic nitric nitrous\n\
azt retrovir zdv zidovudine\n\
azure cerulean\n\
azure cerulean lazuline sapphire\n\
azygos azygous\n\
ba barium\n\
babbler cackler\n\
babbler chatterbox chatterer magpie prater spouter\n\
babe baby infant\n\
babiroussa babirusa babirussa\n\
baboo babu\n\
baby child\n\
babylonia chaldaea chaldea\n\
babyminder minder\n\
babysitter sitter\n\
baccate bacciferous berried\n\
baccate berrylike\n\
bacchanal bacchanalian bacchic carousing orgiastic\n\
bacchanal bacchant\n\
baccy tobacco\n\
bacillar bacillary\n\
bacillar bacillary bacilliform baculiform\n\
bacillariophyceae diatomophyceae\n\
backbiter defamer libeler maligner slanderer traducer vilifier\n\
backbone grit gumption guts moxie sand\n\
backbone rachis spine\n\
backbone spine\n\
backcloth backdrop background\n\
background desktop\n\
backhand backhanded\n\
backmost hindermost hindmost rearmost\n\
backpack haversack knapsack packsack rucksack\n\
backpacker packer\n\
backside rear\n\
backslider recidivist reversionist\n\
backspace backspacer\n\
backstage offstage\n\
backstage offstage wing\n\
backstair backstairs furtive\n\
backstop catcher\n\
backswept sweptback\n\
backsword singlestick\n\
backup relief reliever\n\
backward feebleminded\n\
backwoods boondocks hinterland\n\
backwoodsman frontiersman\n\
bacteria bacterium\n\
bacteriacide bactericide\n\
bactericidal disinfectant germicidal\n\
bacterioid bacterioidal bacteroid bacteroidal\n\
bacteriologic bacteriological\n\
bacteriophage phage\n\
bacteriophagic bacteriophagous\n\
baddie villain\n\
badger wisconsinite\n\
badgerer heckler\n\
badness mischievousness naughtiness\n\
badness severeness severity\n\
baeda beda bede\n\
baffled befuddled bemused bewildered confounded confused mazed\n\
baffling elusive knotty problematic problematical\n\
bagatelle fluff frippery frivolity\n\
bagdad baghdad\n\
bagel beigel\n\
baggage luggage\n\
bagger boxer packer\n\
bagging sacking\n\
baggy sloppy\n\
bagman roadman\n\
bagnio bathhouse\n\
bagnio bawdyhouse bordello brothel cathouse whorehouse\n\
bagpiper piper\n\
baguet baguette\n\
bahrain bahrein\n\
bahraini bahreini\n\
baht tical\n\
baikal baykal\n\
bairiki tarawa\n\
baisa baiza\n\
bait decoy lure\n\
bakeapple cloudberry salmonberry\n\
bakehouse bakery bakeshop\n\
balancer halter haltere\n\
balata beefwood\n\
balaton plattensee\n\
bald barefaced\n\
bald denudate denuded\n\
balder baldr\n\
baldhead baldpate baldy skinhead\n\
baldric baldrick\n\
bale basel basle\n\
baleen whalebone\n\
baleful baneful\n\
baleful forbidding menacing minacious minatory ominous sinister threatening\n\
balefulness maleficence mischief\n\
balibago mahagua mahoe majagua purau\n\
balk baulk\n\
balk baulk rafter\n\
balker baulker noncompliant\n\
balking balky\n\
ball ballock bollock egg nut orchis testicle testis\n\
ball globe orb\n\
balladeer crooner\n\
ballast barretter\n\
ballerina danseuse\n\
ballistocardiograph cardiograph\n\
ballpark park\n\
ballpen ballpoint biro\n\
bally blinking bloody blooming crashing flaming fucking\n\
balm ointment salve unction unguent\n\
balmoral bluebonnet\n\
balmy mild\n\
balsamic balsamy\n\
balthasar balthazar\n\
balusters balustrade banister bannister handrail\n\
bambino toddler tot yearling\n\
banal commonplace hackneyed shopworn threadbare timeworn tired trite\n\
band banding stria striation\n\
band banding stripe\n\
band isthmus\n\
band ring\n\
bandage bind\n\
bandana bandanna\n\
bandeau bra brassiere\n\
bandit brigand\n\
bandoleer bandolier\n\
bandstand stand\n\
bandy bowed bowleg bowlegged\n\
bandyleg bowleg\n\
baneberry cohosh\n\
baneful pernicious pestilent\n\
bang bed bonk eff fuck hump jazz know love screw\n\
bang fringe\n\
bang slam\n\
bang spang\n\
banger cracker firecracker\n\
banging humongous thumping walloping whopping\n\
bangle bauble fallal gaud gewgaw novelty trinket\n\
bangle bracelet\n\
bangtail racehorse\n\
banian banyan\n\
bank camber cant\n\
bankrupt insolvent\n\
banned prohibited\n\
banner standard\n\
banner streamer\n\
banquet feast\n\
banshee banshie\n\
bantam diminutive flyspeck lilliputian midget petite tiny\n\
banteng banting tsine\n\
bantering facetious\n\
baptised baptized\n\
baptistery baptistry font\n\
bar barricade blockade\n\
bar barroom ginmill saloon taproom\n\
bar cake\n\
bar streak stripe\n\
baranduki baronduki barunduki burunduki\n\
barbacan barbican\n\
barbarian barbaric uncivilised uncivilized\n\
barbarian boor churl goth peasant tike tyke\n\
barbarous brutal cruel fell roughshod vicious\n\
barbasco joewood\n\
barbate bearded bewhiskered whiskered whiskery\n\
barbecue barbeque\n\
barbecued grilled\n\
barbed biting mordacious nipping pungent\n\
barbel feeler\n\
barbital barbitone diethylmalonylurea veronal\n\
bareback barebacked\n\
bared bareheaded\n\
barefoot barefooted shoeless\n\
bareness starkness\n\
barf puke vomit vomitus\n\
bargainer dealer monger trader\n\
barge flatboat hoy lighter\n\
bargee bargeman lighterman\n\
barilla glasswort kali kelpwort saltwort\n\
barite barytes\n\
baritone barytone\n\
bark barque\n\
barkeep barkeeper barman bartender mixologist\n\
barker doggie doggy pooch\n\
barley barleycorn\n\
barm yeast\n\
barmy yeasty zestful zesty\n\
barnacle cirriped cirripede\n\
barnstormer playactor trouper\n\
barometric barometrical\n\
baron king magnate mogul tycoon\n\
baronet bart\n\
baronial imposing noble stately\n\
baroque baroqueness\n\
baroque churrigueresco churrigueresque\n\
barosaur barosaurus\n\
barracouta snoek\n\
barrater barrator\n\
barred barricaded blockaded\n\
barrel barrelful\n\
barrel bbl\n\
barrel cask\n\
barrel drum\n\
barreled barrelled\n\
barren bleak desolate stark\n\
barren destitute devoid\n\
barren waste wasteland\n\
barricade roadblock\n\
barrow barrowful\n\
barrow tumulus\n\
barrow wheelbarrow\n\
barye microbar\n\
basal elemental elementary primary\n\
basal radical\n\
baseboard mopboard\n\
baseborn humble lowly\n\
baseless groundless unfounded unwarranted\n\
basement cellar\n\
baseness contemptibility despicability despicableness sordidness\n\
bash bonk bop sock whap whop\n\
bashful blate\n\
basia basra\n\
basic canonic canonical\n\
basic introductory\n\
basic staple\n\
basidiomycota basidiomycotina\n\
basilar basilary\n\
basilicata lucania\n\
basin basinful\n\
basin lavatory washbasin washbowl washstand\n\
basin watershed\n\
basket basketful\n\
basket handbasket\n\
basket hoop\n\
basketeer cager\n\
basketmaker basketweaver\n\
basophil basophile\n\
bass basso\n\
bassarisk cacomistle cacomixle ringtail\n\
bassia kochia\n\
basswood lime linden\n\
basswood linden\n\
bast phloem\n\
bastard bogus fake phoney phony\n\
bastard illegitimate whoreson\n\
bastard mongrel\n\
bastardised bastardized\n\
bastardly misbegot misbegotten spurious\n\
baste basting tacking\n\
baste batter clobber\n\
baste tack\n\
baster tacker\n\
bastion citadel\n\
bastioned fortified\n\
bastnaesite bastnasite\n\
basutoland lesotho\n\
bat chiropteran\n\
bat clobber cream drub lick\n\
bather natator swimmer\n\
batholite batholith pluton\n\
batholithic batholitic\n\
bathometer bathymeter\n\
bathroom can john lav lavatory privy toilet\n\
bathtub tub\n\
bathymetric bathymetrical\n\
bathyscape bathyscaph bathyscaphe\n\
batoidei rajiformes\n\
baton billy billystick nightstick truncheon\n\
baton wand\n\
batsman batter hitter slugger\n\
batswana bechuana tswana\n\
battalion multitude plurality\n\
batten batting\n\
batter buffet\n\
battercake flapcake flapjack griddlecake hotcake pancake\n\
battlefield battleground field\n\
battlefront front\n\
battleful bellicose combative\n\
battlement crenelation crenellation\n\
battlemented castellated castled embattled\n\
battler belligerent combatant fighter scrapper\n\
battleship battlewagon\n\
bawd cocotte cyprian harlot prostitute tart whore\n\
bawdiness lewdness obscenity salaciousness salacity\n\
bawdy ribald\n\
bawler bellower roarer screamer screecher shouter yeller\n\
bay embayment\n\
bayberry candleberry waxberry\n\
baycol cerivastatin\n\
bayrut beirut\n\
bazaar bazar\n\
bazillion billion gazillion jillion million trillion zillion\n\
be beryllium glucinium\n\
beacon lighthouse pharos\n\
bead pearl\n\
beading beadwork\n\
beadlike beady buttonlike buttony\n\
beadsman bedesman\n\
beady gemmed jeweled jewelled sequined spangled spangly\n\
beak bill neb nib pecker\n\
beak honker hooter nozzle schnoz schnozzle snoot snout\n\
beak peck\n\
beam irradiation ray\n\
beam ray\n\
beaming beamy effulgent radiant refulgent\n\
beaming glad\n\
beamish smiling twinkly\n\
beanie beany\n\
beantown boston\n\
bear carry\n\
bearable endurable sufferable supportable\n\
bearberry bearwood chittamwood chittimwood\n\
bearberry winterberry\n\
bearcat binturong\n\
beard byssus\n\
beard whiskers\n\
beardless whiskerless\n\
bearer carrier toter\n\
bearer holder\n\
bearer pallbearer\n\
bearing carriage posture\n\
bearing charge\n\
bearing comportment mien presence\n\
bearskin busby shako\n\
beast brute wildcat wolf\n\
beastliness meanness\n\
beastly bestial brutal brute brutish\n\
beastly hellish\n\
beatable vanquishable vincible\n\
beatified blessed\n\
beau boyfriend swain\n\
beau clotheshorse dandy dude fop sheik swell\n\
beauteousness comeliness fairness loveliness\n\
beautician cosmetician\n\
beauty dish knockout looker lulu mantrap peach ravisher smasher stunner sweetheart\n\
beaver castor\n\
beaver oregonian\n\
beaver stovepipe topper\n\
bebop bop\n\
becoming comely decorous seemly\n\
bed bottom\n\
bed layer\n\
bed seam\n\
bedaub besmear\n\
bedbug chinch\n\
bedchamber bedroom chamber\n\
bedclothes bedding\n\
bedcover bedspread counterpane\n\
bedded stratified\n\
bedding litter\n\
bedewed dewy\n\
bedfast bedrid bedridden\n\
bedframe bedstead\n\
bedlam madhouse nuthouse sanatorium\n\
bedouin beduin\n\
bedraggled derelict dilapidated ramshackle tatterdemalion\n\
bedraggled draggled\n\
bedsit bedsitter\n\
bedwetter wetter\n\
beech beechwood\n\
beef boeuf\n\
beefalo cattalo\n\
beefburger burger hamburger\n\
beefeater yeoman\n\
beefy buirdly burly husky strapping\n\
beehive hive\n\
beelzebub devil lucifer satan\n\
beeper pager\n\
beet beetroot\n\
beetle beetling\n\
beetle mallet\n\
beetleweed coltsfoot galax galaxy wandflower\n\
befogged befuddled\n\
befouled fouled\n\
beggar mendicant\n\
beginner founder\n\
beginner initiate novice tiro tyro\n\
beginning origin root rootage source\n\
begrime bemire colly grime soil\n\
begrimed dingy grimy grubby grungy raunchy\n\
beguiled captivated charmed delighted enthralled entranced\n\
beguilement bewitchery\n\
beguiler charmer\n\
beguiler cheat cheater deceiver slicker trickster\n\
behavior behaviour conduct demeanor demeanour deportment\n\
behavioral behavioural\n\
behaviorist behavioristic behaviourist behaviouristic\n\
behaviorist behaviourist\n\
behead decapitate decollate\n\
beheaded decapitated\n\
behemoth colossus giant goliath monster\n\
behemoth colossus giant heavyweight titan\n\
behmen boehm boehme bohme\n\
beholder observer perceiver percipient\n\
behring bering\n\
beige ecru\n\
beijing peiping peking\n\
being organism\n\
belabor belabour\n\
belarus belorussia byelarus byelorussia\n\
belated late tardy\n\
belau palau pelew\n\
beldam beldame\n\
beldam beldame crone hag witch\n\
belem para\n\
belfry campanile\n\
belgique belgium\n\
belgrade beograd\n\
believability credibility credibleness\n\
believable credible\n\
believer truster\n\
believer worshiper worshipper\n\
belittled diminished\n\
belittling deprecating deprecative deprecatory depreciative depreciatory slighting\n\
bell buzzer doorbell\n\
bell campana\n\
bell chime gong\n\
bellarmine bellarmino\n\
bellarmine greybeard longbeard\n\
bellboy bellhop bellman\n\
bellflower campanula\n\
bellicoseness bellicosity\n\
bellied bellying bulbous bulging bulgy protuberant\n\
belligerent militant warring\n\
belly paunch\n\
bellyacher complainer crybaby grumbler moaner sniveller squawker whiner\n\
bellybutton navel omphalos omphalus umbilicus\n\
belorussian byelorussian\n\
beloved darling\n\
beloved dearest honey love\n\
belowground underground\n\
belt swath\n\
beltless unbelted\n\
beltway bypass ringway\n\
beluga hausen\n\
bema chancel sanctuary\n\
bemused preoccupied\n\
benadryl diphenhydramine\n\
bench terrace\n\
bench workbench\n\
bendability pliability\n\
bendable pliable pliant waxy\n\
bended bent\n\
bending deflection deflexion\n\
bendopa brocadopa larodopa levodopa\n\
benedick benedict\n\
benedictive benedictory\n\
benefactor helper\n\
beneficent benevolent eleemosynary philanthropic\n\
beneficiary donee\n\
benefit welfare\n\
benevolent charitable kindly openhearted sympathetic\n\
benevolent freehearted\n\
benighted nighted\n\
benign benignant\n\
benignancy benignity graciousness\n\
benignant gracious\n\
benin dahomey\n\
benjamin benzoin\n\
benne benni benny sesame\n\
bennie benzedrine\n\
bent crumpled dented\n\
benthal benthic benthonic\n\
benumbed dulled\n\
benweed ragweed ragwort\n\
benzene benzine benzol\n\
benzofuran coumarone cumarone\n\
benzoquinone quinone\n\
beplaster plaster\n\
bereaved bereft grieving mourning sorrowing\n\
bereft lovelorn unbeloved\n\
berg iceberg\n\
bergall cunner\n\
berkelium bk\n\
berm shoulder\n\
bermuda bermudas\n\
bermudan bermudian\n\
bern berne\n\
berra yogi\n\
berretta biretta birretta\n\
berserk berserker\n\
berth bunk\n\
berth moor\n\
berth moor wharf\n\
berth moorage mooring\n\
beseeching imploring pleading\n\
beset encrust incrust\n\
besmirch smirch\n\
bespatter spatter\n\
bespeckle speckle\n\
bespectacled monocled spectacled\n\
bespoke bespoken tailored\n\
bespoken betrothed\n\
best better\n\
best topper\n\
bestir rouse\n\
bestower conferrer donor giver presenter\n\
bestubbled stubbled stubbly\n\
betrayer blabber informer rat squealer\n\
betrayer traitor\n\
better bettor punter wagerer\n\
betting dissipated sporting\n\
betweenbrain diencephalon interbrain thalmencephalon\n\
bevel cant chamfer\n\
bevel chamfer\n\
beverage drink drinkable potable\n\
bewitched ensorcelled\n\
bewitching captivating enchanting enthralling entrancing fascinating\n\
bextra valdecoxib\n\
bh bohrium\n\
bharat india\n\
bhutanese bhutani\n\
bi bismuth\n\
bialy bialystoker\n\
biannual biyearly semiannual\n\
bias diagonal\n\
biased colored coloured slanted\n\
biaural binaural\n\
biaxal biaxate biaxial\n\
biblical scriptural\n\
bibliographic bibliographical\n\
bibliophile booklover\n\
bibliopole bibliopolist\n\
bibliothec librarian\n\
bibliothecal bibliothecarial\n\
bibulous boozy drunken sottish\n\
bicentenary bicentennial\n\
bichloride dichloride\n\
bichromate dichromate\n\
bichrome bicolor bicolored bicolour bicoloured dichromatic\n\
biconvex lenticular lentiform\n\
bicorn bicornate bicorned bicornuate bicornuous\n\
bicorn bicorne\n\
bicuspid bicuspidate\n\
bicuspid premolar\n\
bicycle bike cycle pedal wheel\n\
bicycle bike cycle wheel\n\
bicycler bicyclist biker cyclist wheeler\n\
bida doha\n\
biddy chick\n\
biddy hen\n\
biennial biyearly\n\
biface bifacial\n\
biff pommel pummel\n\
bifurcate biramous branched forficate forked pronged prongy\n\
bigfoot sasquatch\n\
bigger larger\n\
biggish largish\n\
bigheaded persnickety snooty snotty uppish\n\
bighearted bounteous bountiful freehanded giving handsome liberal openhanded\n\
bighorn cimarron\n\
bigmouthed blabbermouthed blabby talkative\n\
bigness largeness\n\
bigwig kingpin\n\
bike motorcycle\n\
bilateral isobilateral\n\
bilateralism bilaterality\n\
bilberry blaeberry whinberry whortleberry\n\
bilberry whortleberry\n\
bile gall\n\
bilestone gallstone\n\
biliary bilious\n\
bilingual bilingualist\n\
bilious liverish livery\n\
biliousness irritability peevishness pettishness snappishness surliness temper\n\
bilirubin haematoidin hematoidin\n\
bilk elude evade\n\
bill billhook\n\
bill eyeshade visor vizor\n\
billboard hoarding\n\
billfish gar garfish garpike\n\
billfish gar needlefish\n\
billfish saury\n\
billfold notecase pocketbook wallet\n\
billow heave surge\n\
billow wallow\n\
billowing billowy surging\n\
billyo billyoh\n\
bilobate bilobated bilobed\n\
bilocular biloculate\n\
bimestrial bimonthly\n\
bimetal bimetallic\n\
bimetallic bimetallistic\n\
bimli kanaf kenaf\n\
bimonthly semimonthly\n\
bin binful\n\
bind truss\n\
bindable bondable\n\
binder ligature\n\
binomial binominal\n\
binuclear binucleate binucleated\n\
bioarm bioweapon\n\
bioflavinoid citrin\n\
biogeographic biogeographical\n\
biographic biographical\n\
biologic biological\n\
bionomic bionomical ecologic ecological\n\
biovular fraternal\n\
biparous twinning\n\
bipartisan bipartizan\n\
biped bipedal\n\
biquadrate biquadratic quartic\n\
birch birchen birken\n\
bird birdie shuttle shuttlecock\n\
bird chick dame doll skirt wench\n\
bird fowl\n\
birdfeeder feeder\n\
birdlime lime\n\
biriani biryani\n\
birl birle\n\
birl spin twirl\n\
birmingham brummagem\n\
birthmark nevus\n\
birthplace cradle provenance provenience\n\
bisayan visayan\n\
biscuit cookie cooky\n\
bise bize\n\
bisexual epicene\n\
bishkek biskek frunze\n\
bishopric diocese episcopate\n\
bister bistre\n\
bistered bistred\n\
bisulcate cloven\n\
bit bite morsel\n\
bit flake fleck scrap\n\
bitch cunt\n\
bitchiness cattiness nastiness spite spitefulness\n\
bitchy cattish catty\n\
bite collation snack\n\
bite pungency raciness\n\
bite sting\n\
biting bitter\n\
bitt bollard\n\
bitter bitterness\n\
bittersweet semisweet\n\
bittersweet waxwork\n\
bitterweed bugloss oxtongue\n\
bitterwood quassia\n\
bittie bitty teensy teentsy teeny wee weensy weeny\n\
bitumenoid bituminoid\n\
bivalent divalent\n\
bivalve bivalved\n\
bivalve lamellibranch pelecypod\n\
bivalvia lamellibranchia\n\
bivouac camp cantonment encampment\n\
bivouac campground campsite encampment\n\
biweekly fortnightly\n\
biweekly semiweekly\n\
bizarre eccentric flakey flaky freakish freaky gonzo outlandish outre\n\
bizarreness outlandishness weirdness\n\
blabbermouth talebearer taleteller tattler tattletale telltale\n\
blabbermouthed leaky talebearing tattling\n\
blackbeard teach thatch\n\
blackbird merl merle ousel ouzel\n\
blackboard chalkboard\n\
blackcap pewit\n\
blackcap thimbleberry\n\
blackdamp chokedamp\n\
blackfish tautog\n\
blackfriar dominican\n\
blackguard bounder cad heel hound\n\
blackguardly rascally roguish scoundrelly\n\
blackhead comedo\n\
blackjack cosh sap\n\
blackleg rat scab strikebreaker\n\
blackmailer extortioner extortionist\n\
blackness inkiness\n\
blackthorn sloe\n\
blacktop blacktopping\n\
bladder vesica\n\
bladderlike bladdery\n\
bladderwrack tang\n\
blade brand steel sword\n\
blade vane\n\
bladelike ensiform swordlike\n\
blamable blameable blameful blameworthy censurable culpable\n\
blame blamed blasted blessed damn damned darned deuced goddam goddamn goddamned infernal\n\
blameless inculpable irreproachable unimpeachable\n\
blanched etiolate etiolated\n\
bland flavorless flavourless insipid savorless savourless vapid\n\
bland politic suave\n\
blandness insipidity insipidness\n\
blandness smoothness suaveness suavity\n\
blank dummy\n\
blank lacuna\n\
blank utter\n\
blanket encompassing extensive panoptic wide\n\
blanquillo tilefish\n\
blaring blasting\n\
blase bored\n\
blase worldly\n\
blasphemous profane\n\
blasphemous profane sacrilegious\n\
blast blow gust\n\
blast boom nail smash\n\
blastemal blastematic blastemic\n\
blaster chargeman\n\
blasting ruinous\n\
blastocele blastocoel blastocoele\n\
blastoderm blastodisc\n\
blastodermatic blastodermic\n\
blastoporal blastoporic\n\
blastosphere blastula\n\
blastospheric blastular\n\
blatant blazing conspicuous\n\
blatant clamant clamorous strident vociferous\n\
blattaria blattodea\n\
blaze brilliance glare\n\
blazing blinding dazzling fulgent glaring glary\n\
bleach whitener\n\
bleached colored coloured dyed\n\
bleached faded washy\n\
bleak cutting\n\
blear bleary\n\
bleary blurred blurry foggy fuzzy hazy muzzy\n\
bleb blister bulla\n\
blebbed blebby\n\
blebby blistery\n\
bleeder haemophile haemophiliac hemophile hemophiliac\n\
blemish deface disfigure\n\
blemish defect mar\n\
blemished flawed\n\
blend immingle intermingle intermix\n\
blende sphalerite\n\
blender liquidiser liquidizer\n\
blessed blest\n\
blighted spoilt\n\
blighter bloke chap cuss fella feller gent lad\n\
blighter cuss gadfly pest pesterer\n\
blimp sausage\n\
blind screen\n\
blind unreasoning\n\
blind unsighted\n\
blinder blinker winker\n\
blindfold blindfolded\n\
blindworm caecilian\n\
blindworm slowworm\n\
blini bliny\n\
blinker flasher\n\
blinking winking\n\
blintz blintze\n\
blistering blistery\n\
blistering hot\n\
blithe blithesome lighthearted lightsome\n\
blixen dinesen\n\
blizzard snowstorm\n\
blob blot fleck\n\
blocadren timolol\n\
blockage closure occlusion stoppage\n\
blocked plugged\n\
blockheaded boneheaded duncical duncish fatheaded loggerheaded thickheaded\n\
blockish blocky\n\
blond blonde\n\
blondness fairness paleness\n\
blood profligate rake rakehell rip roue\n\
bloodberry rougeberry\n\
bloodcurdling nightmarish\n\
bloodhound sleuthhound\n\
bloodiness bloodthirstiness\n\
bloodless exsanguine exsanguinous\n\
bloodline pedigree\n\
bloodroot puccoon redroot tetterwort\n\
bloodstained gory\n\
bloodstone heliotrope\n\
bloodsucker hirudinean leech\n\
bloodsucking leechlike parasitic parasitical\n\
bloodthirsty sanguinary\n\
bloom blossom flower\n\
bloom efflorescence\n\
bloomers drawers knickers pants\n\
blot daub slur smear smirch smudge\n\
blotch splodge splotch\n\
blotched blotchy splotched\n\
blow coke snow\n\
blow float\n\
blowball dandelion\n\
blower cetacean\n\
blowfish globefish puffer pufferfish\n\
blowfish puffer pufferfish\n\
blowgun blowpipe blowtube\n\
blowhard boaster braggart bragger vaunter\n\
blowhole vent venthole\n\
blowlamp blowtorch torch\n\
blown pursy winded\n\
blowpipe blowtube\n\
blowsy blowzy slatternly sluttish\n\
blowup enlargement magnification\n\
blowy breezy windy\n\
bludgeon club\n\
bluebell harebell\n\
bluebill broadbill scaup\n\
bluebottle cornflower\n\
blueing bluing\n\
blueish bluish\n\
bluejacket sailor\n\
bluff bold\n\
blunder fumble\n\
blunderer botcher bumbler bungler butcher fuckup fumbler stumbler\n\
blunt candid forthright frank outspoken plainspoken\n\
blunt stark\n\
blunted dulled\n\
bluntness dullness\n\
blur smear smudge smutch\n\
blurred clouded\n\
blurriness fogginess fuzziness indistinctness softness\n\
blusher paint rouge\n\
blushful blushing\n\
blushful rosy\n\
blusterer loudmouth\n\
blustering blusterous blustery\n\
blustery bullying\n\
bm dejection faeces feces ordure stool\n\
boarder lodger roomer\n\
boastful braggart bragging braggy crowing\n\
boastfulness vainglory\n\
boat sauceboat\n\
boatbill broadbill\n\
boater boatman waterman\n\
boater leghorn panama sailor skimmer\n\
boatload carload shipload\n\
boatswain bosun\n\
bobber bobfloat cork\n\
bobbin reel spool\n\
bobolink reedbird ricebird\n\
bobsled bobsleigh\n\
bobtail bobtailed\n\
bobwhite partridge\n\
boche hun jerry kraut krauthead\n\
boddhisatva bodhisattva\n\
bodensee constance\n\
bodied corporal corporate embodied incarnate\n\
bodiless bodyless\n\
bodiless discorporate disembodied unbodied unembodied\n\
bodily corporal corporeal somatic\n\
bodkin poniard\n\
bodkin threader\n\
body consistence consistency substance\n\
body soundbox\n\
body torso trunk\n\
bodybuilder musclebuilder muscleman\n\
bodyguard escort\n\
bogbean buckbean\n\
bogey bogie bogy\n\
bogeyman booger boogeyman bugaboo bugbear\n\
boggy marshy miry mucky muddy quaggy sloppy sloughy soggy squashy swampy waterlogged\n\
bohemian gipsy gypsy roma romani romany rommany\n\
boil churn moil roil\n\
boiled poached stewed\n\
boiler kettle\n\
boilersuit overall\n\
boisterous fierce\n\
boisterous knockabout\n\
boisterous rambunctious robustious rumbustious unruly\n\
bola bolo\n\
boldness brass cheek face nerve\n\
boldness daring hardihood hardiness\n\
boldness strikingness\n\
bole trunk\n\
bolide fireball\n\
bologram bolograph\n\
bolshevik bolshevist\n\
bolshevik bolshevist bolshevistic\n\
bolshevik bolshie bolshy marxist\n\
bolshy stroppy\n\
bolt deadbolt\n\
bolt thunderbolt\n\
bombard bombardon\n\
bombard pelt\n\
bombardon helicon\n\
bombastic declamatory orotund tumid turgid\n\
bombay mumbai\n\
bomber grinder hero hoagie hoagy sub submarine torpedo zep\n\
bombproof shellproof\n\
bonaparte napoleon\n\
bond hamper shackle trammel\n\
bonderise bonderize\n\
bondmaid bondswoman bondwoman\n\
bondman bondsman\n\
bondsman bondswoman\n\
bonduc chicot\n\
bone ivory pearl\n\
bone os\n\
boned deboned\n\
bonelet ossicle ossiculum\n\
bones castanets clappers\n\
boney bony\n\
boney bony scraggly scraggy scrawny skinny underweight weedy\n\
boniface host innkeeper\n\
boniface winfred wynfrith\n\
boniness bonyness emaciation gauntness maceration\n\
bonnet cowl cowling hood\n\
bonnethead shovelhead\n\
bonnie bonny comely sightly\n\
bonxie skua\n\
bony osseous osteal\n\
boob booby dope dumbbell dummy pinhead\n\
boob bosom breast knocker tit titty\n\
book volume\n\
booked engaged\n\
bookie bookmaker\n\
bookish studious\n\
booklouse deathwatch\n\
bookman scholar student\n\
bookmark bookmarker\n\
bookshop bookstall bookstore\n\
bookworm pedant scholastic\n\
booming flourishing palmy prospering prosperous roaring thriving\n\
booming stentorian\n\
boorish loutish neandertal neanderthal oafish swinish\n\
boorishness uncouthness\n\
boost hike\n\
booster lifter shoplifter\n\
booster plugger promoter\n\
boot trunk\n\
bootblack shoeblack\n\
bootee bootie\n\
booth cubicle kiosk stall\n\
bootleg contraband smuggled\n\
bootleg moonshine\n\
bootlegger moonshiner\n\
bootless fruitless futile sleeveless vain\n\
bootlicker fawner groveler groveller truckler\n\
bootlicking fawning obsequious sycophantic toadyish\n\
bootlicking fawning sycophantic toadyish\n\
booze liquor spirits\n\
boracic boric\n\
borage tailwort\n\
borderland march marchland\n\
borderline delimitation mete\n\
borderline marginal\n\
bore caliber calibre gauge\n\
bore drill\n\
bore dullard\n\
boreal circumboreal\n\
boreas norther northerly\n\
borecole cole colewort kail kale\n\
borer woodborer\n\
boring deadening irksome slow tedious tiresome wearisome\n\
boringness dreariness insipidity insipidness\n\
born innate\n\
borneo kalimantan\n\
borsch borscht borsh borshch borsht bortsch\n\
bosky brushy\n\
bosom embrace hug\n\
bosomy busty buxom curvaceous curvy sonsie sonsy stacked voluptuous\n\
botanic botanical\n\
botanist phytologist\n\
botched bungled\n\
botchy butcherly unskillful\n\
bothered daunted fazed\n\
botonee botonnee\n\
botryoid botryoidal boytrose\n\
bottle bottleful\n\
bottleneck chokepoint constriction\n\
bottom bottomland\n\
bottom freighter merchantman\n\
bottom underside undersurface\n\
bottommost lowermost nethermost\n\
botulin botulismotoxin\n\
botulinum botulinus\n\
bouffant puffy\n\
boulder bowlder\n\
bouldered bouldery rocky stony\n\
boule boulle buhl\n\
bounce bounciness\n\
bounce jounce\n\
bounce rebound recoil resile reverberate ricochet spring\n\
bouncing bouncy peppy spirited zippy\n\
bouncy live lively resilient springy\n\
boundary bounds\n\
boundary limit\n\
bounded delimited\n\
boundedness finiteness finitude\n\
bounder leaper\n\
bounderish lowbred underbred yokelish\n\
boundless limitless unbounded\n\
boundlessness infiniteness infinitude limitlessness unboundedness\n\
bounteousness bounty\n\
bountiful plentiful\n\
bouquet corsage nosegay posy\n\
bouquet fragrance fragrancy redolence sweetness\n\
bourdon drone\n\
bourgeois burgher\n\
bourgeois businessperson\n\
bourgeois conservative materialistic\n\
bourgogne burgundy\n\
bourn bourne\n\
bourtree elderberry\n\
bouse bowse\n\
bovid bovine\n\
bow bowknot\n\
bow crouch stoop\n\
bow fore prow stem\n\
bowdleriser bowdlerizer expurgator\n\
bowed bowing\n\
bowel gut intestine\n\
bowelless cutthroat fierce\n\
bower embower\n\
bowerbird catbird\n\
bowfin dogfish grindle\n\
bowl bowlful\n\
bowl trough\n\
bowler derby\n\
box boxful\n\
box boxwood\n\
box loge\n\
box package\n\
boxberry checkerberry spiceberry teaberry wintergreen\n\
boxberry partridgeberry twinberry\n\
boxer pugilist\n\
boxers boxershorts drawers shorts underdrawers\n\
boxfish trunkfish\n\
boxlike boxy\n\
boy son\n\
boyish boylike schoolboyish\n\
bozo cat guy hombre sod\n\
bozo cuckoo fathead goof goofball goose jackass twat zany\n\
br bromine\n\
braced buttressed\n\
bracelet watchband watchstrap wristband\n\
brachiopod brachiopodous\n\
brachiopod lampshell\n\
brachycephalic brachycranial brachycranic\n\
brachycephalism brachycephaly\n\
brachydactylic brachydactylous\n\
bracing brisk refreshful refreshing\n\
bracken brake\n\
brackish briny\n\
bracteate bracted\n\
bracteole bractlet\n\
bradawl pricker\n\
brage bragi\n\
brahma brahman brahmin\n\
brahman brahmin\n\
brahminic brahminical\n\
braid braiding\n\
braid plait tress\n\
braid pleach\n\
brain brainiac einstein genius mastermind\n\
brain encephalon\n\
braincase brainpan cranium\n\
brainchild inspiration\n\
brainish hotheaded impetuous impulsive madcap tearaway\n\
brainless headless\n\
brainsick crazy demented disturbed mad unbalanced unhinged\n\
braky brambly\n\
braless topless\n\
branch leg ramification\n\
branch offset offshoot outgrowth\n\
branched branching ramate ramose ramous\n\
branchia gill\n\
branchiate gilled\n\
branchiopod branchiopodan\n\
branchiopod branchiopodan branchiopodous\n\
branchlet sprig twig\n\
brand brandmark trademark\n\
brand firebrand\n\
brandish flourish wave\n\
brant brent\n\
brash cheeky nervy\n\
brashness flashiness garishness gaudiness glitz loudness meretriciousness tawdriness\n\
brasier brazier\n\
brasil brazil\n\
brass plaque\n\
brassbound ironclad\n\
brassicaceae cruciferae\n\
brasslike brassy\n\
brat bratwurst\n\
brat terror\n\
bratislava pozsony pressburg\n\
brattish bratty\n\
braunschweig brunswick\n\
brave braw\n\
brave courageous\n\
braveness bravery courage courageousness\n\
brawn brawniness heftiness muscle muscularity sinew\n\
brawny hefty muscular powerful sinewy\n\
brazenness shamelessness\n\
brazilwood peachwood\n\
bread breadstuff\n\
breadbasket stomach tum tummy\n\
breadth width\n\
break bust\n\
break bust wear\n\
break collapse founder give\n\
break fault faulting fracture\n\
breakability fragility frangibility frangibleness\n\
breakaway fissiparous separatist\n\
breakax breakaxe\n\
breaker ledgeman\n\
breakstone rockfoil saxifrage\n\
breakwater bulwark groin groyne jetty mole seawall\n\
breast chest\n\
breast summit\n\
breastbone sternum\n\
breastpin broach brooch\n\
breastwork parapet\n\
breathalyser breathalyzer\n\
breathed voiceless\n\
breather schnorchel schnorkel snorkel\n\
breathing eupneic eupnoeic\n\
breathless breathtaking\n\
breathless dyspneal dyspneic dyspnoeal dyspnoeic\n\
breathless inanimate pulseless\n\
breechcloth breechclout loincloth\n\
breeched pantalooned trousered\n\
breeches knickerbockers knickers\n\
breeding education training\n\
breeding genteelness gentility\n\
breeziness jauntiness\n\
breiz bretagne brittany\n\
breslau wroclaw\n\
breughel bruegel brueghel\n\
brevibloc esmolol\n\
brevicipitidae microhylidae\n\
brevity briefness transience\n\
brew brewage\n\
briar brier\n\
briar brier bullbrier catbrier greenbrier\n\
briar brier eglantine sweetbriar sweetbrier\n\
briarwood brierwood\n\
bribable corruptible dishonest purchasable venal\n\
briber suborner\n\
brickfield brickyard\n\
brickle brickly brittle\n\
bridal nuptial spousal\n\
bride bridget brigid\n\
bridegroom groom\n\
bridge bridgework\n\
bridge nosepiece\n\
bridge span\n\
bridgehead foothold\n\
brier brierpatch\n\
brightness luminance luminosity luminousness\n\
brilliance grandeur grandness magnificence splendor splendour\n\
brilliancy luster lustre splendor splendour\n\
brim lip rim\n\
brimful brimfull brimming\n\
brinded brindle brindled tabby\n\
brine saltwater seawater\n\
bring convey\n\
bring convey fetch\n\
brininess salinity\n\
brink threshold verge\n\
brink verge\n\
briny main\n\
briony bryony\n\
briquet briquette\n\
brisling sprat\n\
bristle uprise\n\
bristliness prickliness spininess thorniness\n\
bristly prickly splenetic waspish\n\
brit britisher briton\n\
brit britt\n\
britain uk\n\
british brits\n\
brittle toffee toffy\n\
brittle unannealed\n\
brittlebush incienso\n\
brittleness crispiness crispness\n\
brno brunn\n\
broadax broadaxe\n\
broadband wideband\n\
broadbill shoveler shoveller\n\
broadcaster spreader\n\
broadness wideness\n\
broadnosed platyrhine platyrhinian platyrrhine platyrrhinian platyrrhinic\n\
broadtail caracul karakul\n\
brobdingnagian huge immense vast\n\
brocaded embossed raised\n\
brogan brogue clodhopper\n\
broiled grilled\n\
broke bust skint\n\
brokenhearted heartbroken heartsick\n\
brolly gamp\n\
bromberg bydgoszcz\n\
brome bromegrass\n\
bromeosin eosin\n\
bromidic corny platitudinal platitudinous\n\
bromoform tribromomethane\n\
bronc broncho bronco\n\
broncobuster buster\n\
bronze bronzy\n\
bronzed suntanned tanned\n\
brooch clasp\n\
brooder incubator\n\
brooding broody contemplative meditative musing pensive pondering reflective ruminative\n\
broody sitter\n\
brook creek\n\
broom heather ling\n\
brother buddy chum crony pal sidekick\n\
brother comrade\n\
brotherlike brotherly fraternal\n\
brow eyebrow supercilium\n\
brow forehead\n\
brow hilltop\n\
brown browned\n\
brown brownish\n\
brown brownness\n\
browne phiz\n\
brownie elf gremlin hob imp pixie pixy\n\
browse graze pasture\n\
browse surf\n\
bruise contuse\n\
bruiser bull samson strapper\n\
brumal hibernal hiemal\n\
brummie brummy\n\
brumous foggy hazy misty\n\
brunet brunette\n\
brunhild brunnhilde brynhild\n\
brusa bursa\n\
brushed fleecy napped\n\
brusk brusque curt\n\
brussels bruxelles\n\
brutal unrelenting\n\
brutality ferociousness savagery viciousness\n\
bryopsida musci\n\
bryozoa polyzoa\n\
bryozoan polyzoan\n\
bubbliness effervescence frothiness\n\
bubbling bubbly effervescing foaming foamy frothy spumy\n\
bubbling effervescent frothy scintillating sparkly\n\
bubbly champagne\n\
buccal oral\n\
buccaneer pirate\n\
bucharest bucharesti bucuresti\n\
buck charge shoot\n\
buck horse sawbuck sawhorse\n\
buckaroo buckeroo vaquero\n\
bucket bucketful\n\
bucket pail\n\
buckeye conker\n\
buckeye ohioan\n\
buckle clasp\n\
buckle crumple\n\
buckle warp\n\
buckler shield\n\
buckminsterfullerene buckyball\n\
buckram starchy\n\
buckthorn ribgrass ribwort\n\
bucolic pastoral\n\
bucolic peasant provincial\n\
buddha gautama siddhartha\n\
buddhist buddhistic\n\
budgereegah budgerigar budgerygah budgie lovebird\n\
buffet counter sideboard\n\
bufflehead butterball dipper\n\
buffoon clown\n\
buffoon clown goof goofball\n\
buffoonish clownish clownlike zany\n\
bug germ microbe\n\
bug hemipteran hemipteron\n\
bugger sod sodomist sodomite\n\
buggy roadster\n\
bugle bugleweed\n\
bugologist entomologist\n\
build habitus physique\n\
builder constructor\n\
building edifice\n\
built reinforced\n\
bujumbura usumbura\n\
bulb lightbulb\n\
bulb medulla\n\
bulbil bulblet\n\
bulblike bulbous\n\
bulge bump excrescence extrusion gibbosity gibbousness hump jut prominence protrusion protuberance swelling\n\
bulge pop protrude\n\
bulghur bulgur\n\
bulginess roundedness\n\
bulging convex\n\
bulk majority\n\
bulk mass volume\n\
bulkiness massiveness\n\
bull cop copper fuzz pig\n\
bull taurus\n\
bullbat nighthawk\n\
bulldozer dozer\n\
bullet slug\n\
bulletproof unassailable unshakable watertight\n\
bullfight corrida\n\
bullfighter toreador\n\
bullheaded pigheaded\n\
bullheadedness obstinacy obstinance pigheadedness stubbornness\n\
bullock steer\n\
bullrush bulrush\n\
bullrush bulrush nailrod reedmace\n\
bully hooligan roughneck rowdy ruffian yob yobbo yobo\n\
bulwark rampart wall\n\
bum cheap cheesy chintzy crummy punk sleazy tinny\n\
bum crumb git lowlife puke rat rotter skunk stinker stinkpot\n\
bum hobo\n\
bum idler layabout loafer\n\
bumble falter stumble\n\
bumblebee humblebee\n\
bumbling bungling butterfingered handless\n\
bump dislodge\n\
bump knock\n\
bumpkin chawbacon hayseed hick rube yahoo yokel\n\
bumpkinly hick rustic unsophisticated\n\
bumptiousness cockiness forwardness pushiness\n\
bumpy jolting jolty jumpy rocky\n\
bunch bundle clump cluster\n\
bunchberry crackerberry\n\
bundle sheaf\n\
bundle wad\n\
bung spile\n\
bungalow cottage\n\
bungling clumsy fumbling incompetent\n\
bunk escape lam scarper scat\n\
bunker dugout\n\
bunker trap\n\
bunsen etna\n\
buoyancy irrepressibility\n\
buoyant chirpy perky\n\
buoyant floaty\n\
bur burr\n\
burbling burbly effusive gushing\n\
burbot cusk eelpout ling\n\
burden burthen weight\n\
burden loading\n\
burdenless unburdened\n\
burdensome onerous taxing\n\
burdensomeness heaviness onerousness oppressiveness\n\
burdock clotbur\n\
bureau chest dresser\n\
buret burette\n\
burgess burgher\n\
burgoo oatmeal\n\
buried inhumed interred\n\
burk burke\n\
burka burqa\n\
burl knot slub\n\
burlap gunny\n\
burma myanmar\n\
burnability combustibility combustibleness\n\
burnable ignitable ignitible\n\
burned burnt\n\
burnish furbish\n\
burnish gloss glossiness polish\n\
burnished lustrous shining shiny\n\
burnoose burnous burnouse\n\
burnside sideburn\n\
burrow tunnel\n\
bursiform pouchlike saclike\n\
burst collapse\n\
burster charge\n\
burundi burundian\n\
bury immerse swallow\n\
bus busbar\n\
bus heap jalopy\n\
bush dubya dubyuh\n\
bush shrub\n\
bushbaby galago\n\
bushbuck guib\n\
bushman khoisan\n\
bushwhacker hillbilly\n\
bushy shaggy\n\
businesslike earnest\n\
buspar buspirone\n\
buss kiss osculate snog\n\
bust rupture\n\
buster dude\n\
bustle hustle\n\
busy busybodied interfering meddlesome meddling officious\n\
busy engaged\n\
busy fussy\n\
busybody quidnunc\n\
butat butut\n\
butazolidin phenylbutazone\n\
butch dike dyke\n\
butch flattop\n\
butch macho\n\
butcher meatman\n\
butcher slaughter\n\
butcher slaughterer\n\
butcherly gory sanguinary sanguineous slaughterous\n\
butene butylene\n\
butler pantryman\n\
butterball fatso fatty\n\
buttercup butterflower crowfoot goldcup kingcup\n\
butterfish stromateid\n\
butterweed ragwort\n\
buttery fulsome oily oleaginous smarmy soapy unctuous\n\
buttery larder pantry\n\
buttock cheek\n\
button clit clitoris\n\
button release\n\
buttoned fastened\n\
buttress buttressing\n\
buxom zaftig zoftig\n\
buyer emptor purchaser vendee\n\
bygone bypast departed foregone\n\
bypass shunt\n\
bypath byroad byway\n\
byre cowbarn cowhouse cowshed\n\
byrnie hauberk\n\
byzantine convoluted involved knotty tangled tortuous\n\
c2h6 ethane\n\
ca calcium\n\
ca california\n\
caaba kaaba\n\
cab cabriolet\n\
cab hack taxi taxicab\n\
cab taxi\n\
cabalist kabbalist\n\
cabalistic cryptic cryptical kabbalistic qabalistic sibylline\n\
cabaret club nightclub nightspot\n\
cabasset morion\n\
cabassous tatouay\n\
cabbage chou\n\
cabby cabdriver cabman taxidriver taximan\n\
cabinet console\n\
cabinet locker\n\
cable line\n\
caboose cookhouse galley\n\
cabstand taxistand\n\
cacatua kakatoe\n\
cacique cazique\n\
cackly squawky\n\
cacodaemon cacodemon\n\
cacodaemonic cacodemonic\n\
cacodyl tetramethyldiarsine\n\
cacogenic dysgenic\n\
cacophonic cacophonous\n\
cacuminal retroflex\n\
cadaver clay corpse remains\n\
cadaveric cadaverous\n\
cadaverous emaciated gaunt haggard pinched skeletal wasted\n\
caddish unchivalrous ungallant\n\
caddisworm strawworm\n\
cadence cadency\n\
cadenced cadent\n\
cadet plebe\n\
cadger mooch moocher scrounger\n\
cadmium cd\n\
caducous shed\n\
caecal cecal\n\
caeciliadae caeciliidae\n\
caecum cecum\n\
caesarean caesarian\n\
caesarean caesarian cesarean cesarian\n\
caesium cesium cs\n\
caespitose cespitose tufted\n\
cafe coffeehouse\n\
caffein caffeine\n\
caffer caffre kaffir kafir\n\
caftan kaftan\n\
cage coop\n\
cagey cagy canny clever\n\
cagey cagy chary\n\
caiman cayman\n\
caimitillo satinleaf\n\
caisson coffer lacunar\n\
caisson cofferdam\n\
cake coat\n\
cake patty\n\
cakehole gob hole maw trap yap\n\
calabash gourd\n\
calabura silkwood\n\
calamari calamary squid\n\
calamine hemimorphite\n\
calamitous disastrous fatal fateful\n\
calamus flagroot\n\
calamus quill\n\
calan isoptin verapamil\n\
calapooya calapuya kalapooia kalapuya\n\
calash caleche\n\
calcaneus heelbone\n\
calcareous chalky\n\
calced shod\n\
calcedony chalcedony\n\
calceiform calceolate\n\
calceolaria slipperwort\n\
calciferol cholecalciferol ergocalciferol viosterol\n\
calcitonin thyrocalcitonin\n\
calculated deliberate measured\n\
calculating calculative conniving scheming shrewd\n\
calculator computer estimator figurer reckoner\n\
calculus concretion\n\
calculus tartar tophus\n\
calcutta kolkata\n\
caldron cauldron\n\
calean chicha hookah kalian narghile nargileh sheesha shisha\n\
calefacient warming\n\
calefaction incalescence\n\
calefactive calefactory\n\
calendered glossy\n\
calendric calendrical\n\
calf calfskin\n\
calf sura\n\
caliber calibre quality\n\
calibrated graduated\n\
caliche hardpan\n\
calicular calycular\n\
caliculus calycle calyculus\n\
calif caliph kalif kaliph khalif khalifah\n\
californium cf\n\
caligula gaius\n\
caliper calliper\n\
calk calkin\n\
calk caulk\n\
caller company\n\
caller phoner telephoner\n\
calligrapher calligraphist\n\
calligraphic calligraphical\n\
calliophis callophis\n\
callipygian callipygous\n\
callosity callousness hardness insensibility unfeelingness\n\
callous calloused thickened\n\
callous indurate pachydermatous\n\
callow fledgling unfledged\n\
calm calmness composure equanimity\n\
calm serene tranquil unagitated\n\
caloric thermal thermic\n\
calorie kilocalorie\n\
calpac calpack kalpac\n\
calpe gibraltar\n\
calumniatory calumnious defamatory denigrating denigrative denigratory libellous libelous slanderous\n\
calvaria skullcap\n\
calvary golgotha\n\
calvinist calvinistic calvinistical\n\
calvinist genevan\n\
calx lime quicklime\n\
calyceal calycinal calycine\n\
calycle calyculus epicalyx\n\
calycled calyculate\n\
camachile huamachil\n\
camaraderie chumminess comradeliness comradery comradeship\n\
camas camash camass camosh quamash\n\
camassia quamassia\n\
cambodia kampuchea\n\
cambodian kampuchean\n\
cambria cymru wales\n\
cambrian cymry welsh welshman\n\
cambrian welsh\n\
camelia camellia\n\
camelopard giraffe\n\
cameraman cinematographer\n\
cameroon cameroun\n\
camion dray\n\
camion lorry\n\
camisole underbodice\n\
camo camouflage\n\
camomile chamomile\n\
camouflage disguise\n\
camp campy\n\
campaigner candidate nominee\n\
campanular campanulate campanulated\n\
campeachy logwood\n\
camphorweed vinegarweed\n\
campion catchfly silene\n\
campong kampong\n\
campylorhynchus heleodytes\n\
can canful\n\
can commode crapper potty stool throne toilet\n\
can tin\n\
canaan palestine\n\
canafistola canafistula\n\
canal channel duct\n\
cananga canangium\n\
canara kanara\n\
canarese kanarese\n\
canary fink sneak sneaker snitch snitcher stoolie stoolpigeon\n\
cancel delete\n\
cancellate cancellated cancellous\n\
cancellate cancellated clathrate\n\
cancelled off\n\
cancer crab\n\
candela candle cd\n\
candelabra candelabrum\n\
candent incandescent\n\
candidate prospect\n\
candidness candor candour directness forthrightness frankness\n\
candied crystalised crystalized glace\n\
candle taper\n\
candy confect\n\
candymaker confectioner\n\
cane flog lambast lambaste\n\
canescent hoary\n\
canicula sirius sothis\n\
canid canine\n\
canine cuspid dogtooth eyetooth\n\
canine laniary\n\
caning wicker wickerwork\n\
canistel eggfruit\n\
canister cannister tin\n\
cankerous ulcerated ulcerous\n\
cannabis ganja marihuana marijuana\n\
cannabis hemp\n\
canned tinned\n\
canned transcribed\n\
cannon shank\n\
cannular tubelike tubular vasiform\n\
cannulate cannulise cannulize canulate intubate\n\
canoeist paddler\n\
canon canyon\n\
canonic canonical\n\
canonic canonical sanctioned\n\
canonised canonized glorified\n\
canorous songful\n\
cant slant tilt\n\
cantabile singing\n\
cantala maguey\n\
cantaloup cantaloupe\n\
cantankerous crotchety ornery\n\
canton guangzhou kuangchou kwangchow\n\
cantor choirmaster precentor\n\
cantor hazan\n\
canute cnut knut\n\
canvas canvass\n\
canvas canvass sail sheet\n\
canvasser headcounter pollster\n\
canvasser scrutineer\n\
canvasser solicitor\n\
caoutchouc rubber\n\
capability capableness\n\
capaciousness commodiousness roominess spaciousness\n\
capacitance capacitor condenser\n\
capacitance capacity\n\
capacity content\n\
caparison housing trapping\n\
cape ness\n\
capelan capelin caplin\n\
capella gallinago\n\
capercaillie capercailzie\n\
capeweed gosmore\n\
capibara capybara\n\
capillary hairlike\n\
capital chapiter\n\
capital great majuscule\n\
capitalist capitalistic\n\
capitular capitulary\n\
capitulum ear spike\n\
capone scarface\n\
capoten captopril\n\
cappelletti ravioli\n\
capricious freakish\n\
capricious impulsive whimsical\n\
capriciousness unpredictability\n\
capricorn capricornus\n\
capricorn goat\n\
caprimulgid goatsucker nightjar\n\
capsicum pepper\n\
capsid mirid\n\
capsidae miridae\n\
capsize turtle\n\
capstone copestone stretcher\n\
capsulate capsulated\n\
capsulate capsule capsulise capsulize\n\
captain chieftain\n\
captain headwaiter\n\
captain skipper\n\
captious faultfinding\n\
captivated charmed\n\
captive confined imprisoned jailed\n\
captive prisoner\n\
captor capturer\n\
capuchin ringtail\n\
car gondola\n\
car railcar\n\
carabineer carabinier carbineer\n\
carabiner karabiner\n\
carack carrack\n\
carafate sucralfate\n\
carafe decanter\n\
carageen carrageen carragheen\n\
caranda caranday\n\
carapace cuticle shell shield\n\
carat karat kt\n\
caravan van\n\
caravansary caravanserai khan\n\
carbamide urea\n\
carbohydrate saccharide sugar\n\
carbonaceous carbonic carboniferous carbonous\n\
carbonyl carbonylic\n\
carboxyl carboxylic\n\
carbuncled carbuncular\n\
carburetor carburettor\n\
carcajou wolverine\n\
carcase carcass\n\
carcharias odontaspis\n\
carchariidae odontaspididae\n\
card tease\n\
card wag wit\n\
cardamom cardamon\n\
cardamom cardamon cardamum\n\
cardboard unlifelike\n\
cardcastle cardhouse\n\
cardinal carmine\n\
cardinal central fundamental primal\n\
cardinal redbird\n\
cardiograph electrocardiograph\n\
cardiopulmonary cardiorespiratory\n\
cardizem diltiazem\n\
cardsharp cardsharper sharper sharpie sharpy\n\
cardura doxazosin\n\
careen keel lurch reel stagger swag\n\
careen tilt wobble\n\
carefree freewheeling slaphappy\n\
carefree unworried\n\
careful deliberate measured\n\
careful heedful\n\
careful thrifty\n\
carefulness caution cautiousness\n\
caregiver pcp\n\
careless regardless\n\
carelessness sloppiness\n\
carelian karelian\n\
caress fondle\n\
careworn drawn haggard raddled worn\n\
cargo consignment freight lading loading payload shipment\n\
caribe pirana piranha\n\
caribou reindeer\n\
carinate carinated keeled ridged\n\
carlos salim sanchez taurus\n\
carlovingian carolingian\n\
carmine cerise cherry crimson reddish ruby ruddy scarlet\n\
carnation gillyflower\n\
carnelian cornelian\n\
carolean caroline\n\
caroler caroller\n\
carolina carolinas\n\
carolus charlemagne charles\n\
carotene carotin\n\
carousel carrousel\n\
carousel carrousel roundabout whirligig\n\
carouser wassailer\n\
carpellate pistillate\n\
carper niggler\n\
carpet carpeting rug\n\
carpetbag carpetbagging\n\
carpus wrist\n\
carrageenan carrageenin\n\
carrefour crossing crossroad crossway intersection\n\
carrel carrell cubicle stall\n\
carriage coach\n\
carriage equipage\n\
carriage perambulator pram pushchair stroller\n\
carrier flattop\n\
carrier mailman postman\n\
carrier newsboy\n\
carroll dodgson\n\
carrottop redhead redheader\n\
carry channel conduct convey impart transmit\n\
carry dribble\n\
carry transport\n\
carryall holdall tote\n\
cart drag hale haul\n\
cart handcart pushcart\n\
carthaginian punic\n\
carthorse drayhorse\n\
cartilage gristle\n\
cartilaginous gristly rubbery\n\
cartographic cartographical\n\
carton cartonful\n\
cartouch cartouche\n\
cartridge magazine\n\
cartridge pickup\n\
caruncle caruncula\n\
caruncular carunculous\n\
carunculate carunculated\n\
carved carven\n\
carver cutter\n\
carver sculptor sculpturer\n\
carver woodcarver\n\
caryophyllales chenopodiales\n\
caryopsis grain\n\
casava cassava\n\
casbah kasbah\n\
casebook textbook\n\
cased encased incased\n\
cashable redeemable\n\
cashbox till\n\
cashier teller\n\
cashmere kashmir\n\
casing shell\n\
cask caskful\n\
casket coffin\n\
caspar gaspar\n\
casquet casquetel\n\
cassava manioc\n\
cassava manioc manioca\n\
cassie huisache\n\
cassite kassite\n\
castaway ishmael outcast pariah\n\
casteless outcaste\n\
caster castor\n\
castile castilla\n\
castilleia castilleja\n\
castle palace\n\
castle rook\n\
castrate eunuch\n\
castrated unsexed\n\
casualness familiarity\n\
casuist sophist\n\
casuistic casuistical\n\
cat caterpillar\n\
cat kat khat qat quat\n\
catabatic katabatic\n\
catabolic katabolic\n\
catachrestic catachrestical\n\
cataclysm catastrophe\n\
cataclysmal cataclysmic\n\
catacorner catercorner\n\
cataloger cataloguer\n\
catamenial menstrual\n\
catamount cougar painter panther puma\n\
catamount lynx\n\
cataphoretic electrophoretic\n\
cataplasm plaster poultice\n\
catapres clonidine\n\
catapult launcher\n\
catapult sling\n\
catapult sling slingshot\n\
catapultian catapultic\n\
catarrhine catarrhinian\n\
catastrophic ruinous\n\
catchfly lychnis\n\
catching communicable contagious contractable transmissible transmittable\n\
catchweed cleavers clivers\n\
catchy tricky\n\
catechetic catechetical\n\
catechetic catechistic\n\
catechumen neophyte\n\
categoric categorical\n\
categoric categorical unconditional\n\
categorised categorized\n\
catenate catenulate\n\
catenulate chainlike\n\
catfish mudcat\n\
catfish wolffish\n\
catgut gut\n\
cathartic evacuant purgative\n\
cathartic psychotherapeutic\n\
cathartic releasing\n\
cathay china prc\n\
cathedral duomo\n\
catholicity universality\n\
catholicon nostrum panacea\n\
catmint catnip\n\
catoptric catoptrical\n\
catsup cetchup ketchup\n\
cattie catty\n\
cattle cows kine oxen\n\
cattleman cowboy cowhand cowherd cowman cowpoke cowpuncher puncher\n\
caucasia caucasus\n\
caucasian caucasic\n\
caucasian caucasoid\n\
caudal taillike\n\
caudata urodella\n\
caudate caudated\n\
caudate urodele\n\
caul veil\n\
caulescent cauline stemmed\n\
caulk caulking\n\
causeless fortuitous uncaused\n\
causeless reasonless\n\
caustic corrosive erosive mordant vitriolic\n\
cauterant cautery\n\
caution circumspection\n\
cautionary prophylactic\n\
cautious conservative\n\
cavalier chevalier\n\
cavalier royalist\n\
cavalla cero\n\
cavalryman trooper\n\
cave undermine\n\
caveman troglodyte\n\
cavernous erectile\n\
caviar caviare\n\
caviler caviller pettifogger quibbler\n\
cavity cavum\n\
cavort disport frisk frolic gambol lark rollick romp skylark sport\n\
cayenne jalapeno\n\
cbr cmb cmbr\n\
cc mil milliliter millilitre ml\n\
ce cerium\n\
ceaseless constant incessant perpetual unceasing unremitting\n\
ceaselessness continuousness incessancy incessantness\n\
cedar cedarwood\n\
cefadroxil ultracef\n\
cefobid cefoperazone\n\
cefotaxime claforan\n\
ceftazidime fortaz tazicef\n\
ceftin cefuroxime zinacef\n\
ceftriaxone rocephin\n\
celandine jewelweed\n\
celandine swallowwort\n\
celebes sulawesi\n\
celebrant celebrater celebrator\n\
celebrated famed famous illustrious notable noted renowned\n\
celebrated historied storied\n\
celebrex celecoxib\n\
celerity quickness rapidity rapidness speediness\n\
celestial ethereal supernal\n\
celestial heavenly\n\
celiac coeliac\n\
celibate continent\n\
cell cellphone\n\
cell cubicle\n\
cellaret minibar\n\
cellblock ward\n\
cellist violoncellist\n\
cello violoncello\n\
celluloid synthetic\n\
celom celoma coelom\n\
celt kelt\n\
celtic gaelic\n\
cembalo harpsichord\n\
cement cementum\n\
cemetery graveyard necropolis\n\
cenobite coenobite\n\
cenobitic cenobitical coenobitic coenobitical\n\
censer thurible\n\
cental centner cwt hundredweight quintal\n\
centaur centaurus\n\
centenary centennial\n\
centerboard centreboard\n\
centerpiece centrepiece\n\
centiliter centilitre cl\n\
centimeter centimetre cm\n\
centner doppelzentner hundredweight\n\
central exchange\n\
centralised centralized\n\
centralising centralizing\n\
centralist centralistic\n\
centrarchid sunfish\n\
centre eye heart middle\n\
centre midpoint\n\
centric centrical\n\
centrifugal motor\n\
centrifugate centrifuge\n\
centrifuge extractor separator\n\
centripetal receptive sensory\n\
centripetal unifying\n\
centrist moderate moderationist\n\
centromere kinetochore\n\
cephalaspid osteostracan\n\
cephalaspida osteostraci\n\
cephalexin keflex keflin keftab\n\
cephaloglycin kafocin\n\
cephalopod cephalopodan\n\
cephalosporin mefoxin\n\
ceramicist ceramist potter thrower\n\
ceratin keratin\n\
ceratosaur ceratosaurus\n\
cerberus hellhound\n\
cereal grain\n\
cerebral intellectual\n\
cerement pall shroud\n\
ceremonious conventional\n\
ceremonious pompous\n\
ceriman monstera\n\
cerise cherry\n\
cernuous drooping nodding pendulous weeping\n\
cero kingfish pintado\n\
certain sealed\n\
certain sure\n\
certifiable certified\n\
certified qualified\n\
cerumen earwax\n\
cervid deer\n\
cervix neck\n\
cesspit cesspool sump\n\
cestode tapeworm\n\
cetacean cetaceous\n\
cewa chewa chichewa\n\
ceylonite pleonaste\n\
cfc chlorofluorocarbon\n\
chabasite chabazite\n\
chachka tchotchke tchotchkeleh tsatske tshatshke\n\
chachka tchotchke tsatske tshatshke\n\
chad tchad\n\
chadar chaddar chador chuddar\n\
chadlock charlock\n\
chaetognathan chaetognathous\n\
chafe excoriate\n\
chafe fray fret rub\n\
chafed galled\n\
chaff husk shuck stalk straw stubble\n\
chafflike chaffy\n\
chain strand string\n\
chained enchained\n\
chains irons\n\
chair chairman chairperson chairwoman president\n\
chaise daybed\n\
chaise shay\n\
chalcedon kadikoy\n\
chalcid chalcidfly\n\
chalcidae chalcididae\n\
chaldaea chaldea\n\
chaldaean chaldean chaldee\n\
chalice goblet\n\
chalk deoxyephedrine glass ice meth methamphetamine methedrine shabu trash\n\
chalkstone tophus\n\
challah hallah\n\
challenger competition competitor contender rival\n\
challenging intriguing\n\
chalybite siderite\n\
chamaeleon chameleon\n\
chamaeleonidae chamaeleontidae rhiptoglossa\n\
chamberpot potty\n\
chamfer chase furrow\n\
chamfron chanfron frontstall testiere\n\
chammy chamois shammy\n\
champ champion\n\
champaign field\n\
champion fighter hero paladin\n\
champion prizewinning\n\
champleve cloisonne\n\
chanal chanar\n\
chance fortune hazard luck\n\
chance probability\n\
chanceful chancy dicey dodgy\n\
chancellor premier\n\
chancy flukey fluky iffy\n\
chandelier pendant pendent\n\
chang changjiang yangtze\n\
changan hsian sian singan xian\n\
change variety\n\
changeability changeableness\n\
changeable changeful\n\
changeable chatoyant iridescent\n\
changeable mutable\n\
changeable uncertain unsettled\n\
changefulness inconstancy\n\
changeless constant invariant unvarying\n\
changeless immutable\n\
changeless unalterable\n\
changelessness unchangeability unchangeableness unchangingness\n\
changeling cretin idiot imbecile moron retard\n\
changer modifier\n\
channel channelise channelize transmit transport\n\
channel groove\n\
channelise channelize\n\
channelise channelize maneuver manoeuver manoeuvre steer\n\
chantarelle chanterelle\n\
chantlike intoned singsong\n\
chaotic disorderly\n\
chap cranny crevice fissure\n\
chapati chapatti\n\
chapeau hat lid\n\
chapelgoer nonconformist\n\
chaperon chaperone\n\
chapfallen chopfallen crestfallen deflated\n\
chaplet coronal garland lei wreath\n\
chapped cracked roughened\n\
char charr\n\
char charwoman woman\n\
characid characin\n\
character eccentric type\n\
character fiber fibre\n\
characterless nondescript\n\
charcoal fusain\n\
chargeable indictable\n\
charged supercharged\n\
charger courser\n\
chari shari\n\
chariness wariness\n\
charismatic magnetic\n\
charlatan mountebank\n\
charmer smoothie smoothy\n\
charming magic magical sorcerous witching wizard wizardly\n\
charnel ghastly sepulchral\n\
chartaceous paperlike papery\n\
chartered hired leased\n\
chartless uncharted unmapped\n\
chase tag trail\n\
chased pursued\n\
chaser pursuer\n\
chasid chassid hasid hassid\n\
chasidic chassidic hasidic hassidic\n\
chasse sashay\n\
chasteness restraint simpleness simplicity\n\
chastity virtue\n\
chatterer cotinga\n\
chatty gabby garrulous loquacious talkative talky\n\
chatty gossipy newsy\n\
chaulmoogra chaulmugra\n\
chauvinism jingoism superpatriotism ultranationalism\n\
chauvinist jingo jingoist patrioteer\n\
chauvinistic jingoistic nationalistic superpatriotic ultranationalistic\n\
chaw chew cud quid wad\n\
cheap chinchy chintzy\n\
cheap inexpensive\n\
cheapjack shoddy tawdry\n\
cheapness sleaze tackiness tat\n\
cheapskate tightwad\n\
cheat chess\n\
cheat darnel tare\n\
cheating unsporting unsportsmanlike\n\
chechenia chechnya\n\
checked checkered chequered\n\
checker chequer\n\
checkerberry groundberry teaberry wintergreen\n\
cheekbone malar zygomatic\n\
cheekiness crust freshness gall impertinence impudence insolence\n\
cheer cheerfulness sunniness sunshine\n\
cheerful pollyannaish upbeat\n\
cheering comforting satisfying\n\
cheerless depressing uncheerful\n\
cheery sunny\n\
cheese cheeseflower\n\
cheeseparing skinny\n\
cheetah chetah\n\
cheewink chewink\n\
chekhov chekov\n\
chela claw nipper pincer\n\
chelate chelated\n\
cheliceral chelicerate\n\
chelonethida pseudoscorpiones pseudoscorpionida\n\
chelonia testudinata testudines\n\
chelonidae cheloniidae\n\
chemic chemical\n\
chemise shimmy teddy\n\
chemisorptive chemosorptive\n\
chemotherapeutic chemotherapeutical\n\
chemulpo incheon inchon\n\
chenfish kingfish\n\
chennai madras\n\
cheops khufu\n\
cheremis cheremiss mari\n\
cherimolla cherimoya\n\
cherished precious treasured wanted\n\
chermidae psyllidae\n\
chest pectus thorax\n\
chewable cuttable\n\
chiasm chiasma decussation\n\
chiasmal chiasmatic chiasmic\n\
chic chichi chicness modishness smartness stylishness swank\n\
chic smart voguish\n\
chicken chickenhearted\n\
chicken crybaby wimp\n\
chicken poulet volaille\n\
chickpea garbanzo\n\
chico marx\n\
chicory succory\n\
chief chieftain headman\n\
chief foreman gaffer honcho\n\
chief main primary principal\n\
chiffonier commode\n\
chigetai dziggetai\n\
chigger chigoe\n\
chigger jigger redbug\n\
child fry kid minor nestling nipper shaver tiddler tike tyke youngster\n\
child kid\n\
childish infantile\n\
childishness puerility\n\
childlike childly\n\
chile chili chilli chilly\n\
chiliast millenarian millenarist\n\
chiliastic millenarian\n\
chill gelidity iciness\n\
chilliness coldness coolness frigidity frigidness iciness\n\
chilliness coolness\n\
chilling scary shivery shuddery\n\
chilly parky\n\
chiluba luba\n\
chimaera chimera\n\
chimeral chimeric chimerical\n\
chimneypiece mantel mantelpiece mantlepiece\n\
chimneysweep chimneysweeper\n\
chimp chimpanzee\n\
chin mentum\n\
china chinaware\n\
china taiwan\n\
chinaberry jaboncillo\n\
chinaman chink\n\
chincapin chinkapin chinquapin\n\
chinchillon viscacha\n\
chinchona cinchona\n\
chinese formosan taiwanese\n\
chios khios\n\
chipboard hardboard\n\
chipper debonair debonaire jaunty\n\
chippewa ojibwa ojibway\n\
chips fries\n\
chiromancer palmist palmister\n\
chiropodist podiatrist\n\
chiseler chiseller defrauder gouger grifter scammer swindler\n\
chisinau kishinev\n\
chitlings chitlins chitterlings\n\
chiton polyplacophore\n\
chittamwood chittimwood shittimwood\n\
chivalric knightly medieval\n\
chivalrous knightly\n\
chivalry gallantry politesse\n\
chive chives cive schnittlaugh\n\
chlamyphore pichiciago pichiciego\n\
chlamys perianth perigone perigonium\n\
chlorambucil leukeran\n\
chloramphenicol chloromycetin\n\
chlordiazepoxide libritabs librium\n\
chlorine cl\n\
chloroform trichloromethane\n\
chlorophyl chlorophyll\n\
chlorophyllose chlorophyllous\n\
chloropicrin nitrochloroform\n\
chlorothiazide diuril\n\
chlorotic greensick\n\
chlorpromazine thorazine\n\
chlorthalidone hygroton thalidone\n\
chockablock chockful\n\
chocolate cocoa\n\
chocolate coffee umber\n\
choice prime prize quality select\n\
choiceness fineness\n\
choke clog congest\n\
choke fret gag\n\
choke scrag\n\
choked clogged\n\
chokehold stranglehold throttlehold\n\
choker collar neckband\n\
choker garroter garrotter strangler throttler\n\
choker ruff\n\
chokey choky\n\
choleric hotheaded irascible\n\
choleric irascible\n\
cholesterin cholesterol\n\
chondriosome mitochondrion\n\
chondritic granular\n\
chongqing chungking\n\
chooser picker selector\n\
choosey choosy\n\
chop hack\n\
chophouse steakhouse\n\
chopine platform\n\
chopped shredded sliced\n\
chopper cleaver\n\
chopper eggbeater helicopter whirlybird\n\
chopper pearly\n\
choppy jerky\n\
chordamesoderm chordomesoderm\n\
chorine showgirl\n\
chow chuck eats grub\n\
chrism chrisom\n\
christ deliverer jesus redeemer savior saviour\n\
christ messiah\n\
christiania oslo\n\
christless nonchristian\n\
christlike christly\n\
christmasberry tollon toyon\n\
chroma intensity saturation vividness\n\
chromaticity hue\n\
chromatographic chromatographical\n\
chromium cr\n\
chronic continuing\n\
chronic inveterate\n\
chrysophyceae heterokontae\n\
chthonian chthonic nether\n\
chubbiness pudginess rolypoliness tubbiness\n\
chubby embonpoint plump\n\
chuck pat\n\
chuck toss\n\
chuckhole pothole\n\
chummy matey pally\n\
chump fool gull mug patsy sucker\n\
chunga seriema\n\
chunk lump\n\
chunky dumpy squat squatty stumpy\n\
chunky lumpy\n\
churchman cleric ecclesiastic\n\
churl crosspatch grouch grump\n\
churl niggard scrooge skinflint\n\
churning roiled roiling roily turbulent\n\
chute parachute\n\
chute slide slideway\n\
chutzpa chutzpah hutzpah\n\
chylaceous chylous\n\
chylifactive chylifactory chylific\n\
chymosin rennin\n\
ci curie\n\
cialis tadalafil\n\
cicada cicala\n\
cicatrise cicatrize\n\
cicero tully\n\
cider cyder\n\
cigaret cigarette fag\n\
cigarfish quiaquia\n\
cilantro coriander\n\
cilial ciliary ciliate\n\
ciliary ciliate\n\
ciliata ciliophora\n\
ciliate ciliated\n\
ciliate ciliophoran\n\
cilioflagellata dinoflagellata\n\
cilium eyelash lash\n\
cimetidine tagamet\n\
cinch girth\n\
cincture girdle sash waistband waistcloth\n\
cinder clinker\n\
cinerarium columbarium\n\
cinnabar vermilion vermillion\n\
cinnamene phenylethylene styrene vinylbenzene\n\
cinque fin five fivesome pentad phoebe quint quintet quintuplet\n\
cipher cypher nobody nonentity\n\
cipher cypher nought zero\n\
cipro ciprofloxacin\n\
circle encircle\n\
circle roach roofy rope rophy\n\
circle rotary roundabout\n\
circuit circumference\n\
circuitous devious roundabout\n\
circuitous roundabout\n\
circular orbitual rotary\n\
circularise circularize\n\
circulate distribute\n\
circulative circulatory\n\
circumference perimeter\n\
circumferent encompassing surrounding\n\
circumnavigate compass\n\
circumscribed limited\n\
circumspect discreet\n\
circumvolve rotate\n\
cirque corrie cwm\n\
cirrhus cirrus\n\
cisalpine ultramontane\n\
cislunar sublunar sublunary\n\
cissy effeminate emasculate epicene sissified sissy sissyish\n\
cistercian trappist\n\
cistern cisterna\n\
cistron factor gene\n\
citellus spermophilus\n\
cither cithern citole cittern gittern\n\
cither zither zithern\n\
citified cityfied\n\
citronwood sandarac\n\
city metropolis\n\
civic civil\n\
civies civvies\n\
civil polite\n\
civilisation civilization refinement\n\
civilised civilized\n\
civilised civilized cultivated cultured genteel polite\n\
clad clothed\n\
cladding facing\n\
cladode cladophyll phylloclad phylloclade\n\
claim title\n\
clairvoyant precognitive\n\
clamant crying exigent insistent instant\n\
clamber scramble shin shinny sputter struggle\n\
clammy dank\n\
clamp clinch\n\
clamshell grapple\n\
clandestine surreptitious undercover underground\n\
clanging clangorous\n\
clannish cliquish clubby snobbish snobby\n\
clannishness cliquishness exclusiveness\n\
clansman clanswoman\n\
clapboard weatherboard weatherboarding\n\
clapper glossa lingua tongue\n\
clapper tongue\n\
clarifying elucidative\n\
clarinetist clarinettist\n\
clarity clearness limpidity lucidity lucidness pellucidity\n\
clarity clearness uncloudedness\n\
clash collide\n\
classic classical\n\
classifiable distinctive\n\
classless egalitarian\n\
classmate schoolfellow schoolmate\n\
classroom schoolroom\n\
classy posh swish\n\
clavicle collarbone\n\
clavier fingerboard\n\
clavier klavier\n\
claw hook\n\
clawed taloned\n\
claxon klaxon\n\
clay mud\n\
clayey cloggy\n\
clayware pottery\n\
cleaner cleanser\n\
cleansing purifying\n\
clearance headroom headway\n\
clearcutness preciseness\n\
clearing glade\n\
clearstory clerestory\n\
clearweed richweed\n\
cleave rive split\n\
cleft crevice fissure scissure\n\
cleft dissected\n\
cleg clegg horsefly\n\
cleistocarp cleistothecium\n\
cleistogamic cleistogamous\n\
clench clinch\n\
clenched clinched\n\
cleome spiderflower\n\
clergyman reverend\n\
clerk salesclerk\n\
clever cunning ingenious\n\
cleverness ingeniousness ingenuity\n\
clew clue\n\
click detent pawl\n\
click flick\n\
client customer\n\
client guest node\n\
climatic climatical\n\
climbable surmountable\n\
climber crampon crampoon\n\
climber mounter\n\
cling clingstone\n\
cling hang\n\
clink gaol jail jailhouse pokey poky slammer\n\
clinometer inclinometer\n\
clinoril sulindac\n\
clinquant tinseled tinselly\n\
clioquinol iodochlorhydroxyquin\n\
clip jog trot\n\
clip lop prune snip\n\
clip magazine\n\
clip snip\n\
clipper limiter\n\
clitoral clitoric\n\
cloaca sewer sewerage\n\
cloak clothe drape robe\n\
cloaked clothed draped mantled wrapped\n\
cloaked disguised masked\n\
cloakmaker furrier\n\
cloakroom coatroom\n\
clochard drifter floater vagabond vagrant\n\
clockmaker clocksmith\n\
clod gawk goon lout lubber lummox lump oaf stumblebum\n\
cloddish doltish\n\
clog geta patten sabot\n\
clogged clotted\n\
clogging hindering impeding obstructive\n\
cloistered cloistral conventual monastic monastical\n\
cloistered reclusive secluded sequestered\n\
clomid clomiphene\n\
clomp clump\n\
clone knockoff\n\
clone ringer\n\
closed shut\n\
closed shut unopen\n\
closed unsympathetic\n\
closefisted hardfisted tightfisted\n\
closelipped closemouthed secretive tightlipped\n\
closer finisher\n\
closet cupboard\n\
closet loo\n\
closet press wardrobe\n\
clostridia clostridium\n\
clot coagulum\n\
cloth fabric material textile\n\
clothesless garmentless raimentless\n\
clothier haberdasher\n\
clothing habiliment vesture wear wearable\n\
clotho klotho\n\
cloud corrupt defile sully taint\n\
cloudburst deluge downpour pelter soaker torrent waterspout\n\
clouded overcast sunless\n\
cloudiness muddiness murkiness\n\
cloudless unclouded\n\
cloudlike nebular\n\
cloudy mirky muddy murky turbid\n\
cloudy nebulose nebulous\n\
clover trefoil\n\
cloying saccharine syrupy treacly\n\
clozapine clozaril\n\
club clubhouse\n\
club golfclub\n\
clubable clubbable\n\
clubbish clubby\n\
clubfooted taliped\n\
clump cluster constellate flock\n\
clumsy clunky gawky ungainly unwieldy\n\
clusiaceae guttiferae\n\
clutch prehend seize\n\
cluttered littered\n\
cm curium\n\
cmv cytomegalovirus\n\
cnidaria coelenterata\n\
cnidarian coelenterate\n\
cnossos cnossus knossos\n\
co cobalt\n\
co colorado\n\
coach handler manager\n\
coach tutor\n\
coachwhip ocotillo\n\
coagulant coagulator\n\
coagulate coagulated curdled grumose grumous\n\
coagulated solidified\n\
coal ember\n\
coalbin coalhole\n\
coalescent coalescing\n\
coapt conglutinate\n\
coarse harsh\n\
coarse uncouth vulgar\n\
coarseness commonness grossness raunch vulgarism vulgarity\n\
coarseness graininess granularity\n\
coarseness nubbiness tweediness\n\
coast seacoast seashore\n\
coat coating\n\
coat pelage\n\
coat surface\n\
coating finish finishing\n\
coatrack hatrack\n\
coaxal coaxial\n\
coaxer wheedler\n\
coaxing ingratiatory\n\
cob cobnut filbert hazelnut\n\
cobalamin cyanocobalamin\n\
cobble cobblestone\n\
cobble cobblestone sett\n\
cobbler shoemaker\n\
cobnut filbert\n\
cobweb gossamer\n\
cobwebby diaphanous filmy gauzy gossamer transparent vaporous vapourous\n\
cocain cocaine\n\
cocci coccus\n\
coccidium eimeria\n\
cockamamie cockamamy\n\
cockateel cockatiel\n\
cockle crumple knit pucker rumple\n\
cockle riffle ripple undulate\n\
cocklebur cockleburr\n\
cockroach roach\n\
cockscomb comb coxcomb\n\
cockscomb coxcomb\n\
cocksfoot cockspur\n\
cocksure overconfident\n\
coco coconut\n\
cocoanut coconut\n\
coconspirator conspirator machinator plotter\n\
cocopa cocopah\n\
cocoswood cocuswood\n\
cocoyam dasheen edda taro\n\
cocoyam dasheen eddo taro\n\
cod codfish\n\
cod collect\n\
cod pod seedcase\n\
coddler mollycoddler pamperer spoiler\n\
coder programmer\n\
codetalker windtalker\n\
codified statute\n\
coerebidae dacninae\n\
coetaneous coeval contemporaneous\n\
coeval contemporary\n\
coexistent coexisting\n\
coextensive conterminous coterminous\n\
coffee java\n\
cog sprocket\n\
cogency rigor rigour validity\n\
cogent telling weighty\n\
coggle dodder paddle toddle totter waddle\n\
coggle wobble\n\
cogitable ponderable\n\
cognate connate\n\
cognate sib\n\
cognisable cognizable cognoscible knowable\n\
cognition knowledge noesis\n\
cognoscente connoisseur\n\
cogwheel gear\n\
coherence coherency\n\
coherent consistent logical ordered\n\
coherent logical lucid\n\
coherent tenacious\n\
cohesiveness glueyness gluiness gumminess ropiness tackiness viscidity viscidness\n\
coho cohoe\n\
coif coiffure hairdo hairstyle\n\
coign coigne quoin\n\
coil curl curlicue gyre ringlet scroll whorl\n\
coil curl loop\n\
coil gyrate\n\
coil helix volute whorl\n\
coiling helical spiraling turbinate volute voluted whorled\n\
coincidence concurrence conjunction\n\
coincident coincidental coinciding concurrent cooccurring simultaneous\n\
coiner minter moneyer\n\
coital copulatory\n\
cola dope\n\
colander cullender\n\
cold coldness frigidity frigidness\n\
cold dusty stale\n\
cold frigid\n\
cold inhuman insensate\n\
coldcock deck dump floor\n\
cole kail kale\n\
coleridgean coleridgian\n\
coleslaw slaw\n\
colicky flatulent gassy\n\
collaborationist collaborator quisling\n\
collaborator confederate henchman\n\
collaborator cooperator pardner partner\n\
collage montage\n\
collagenic collagenous\n\
collapsable collapsible\n\
collapse crumble crumple tumble\n\
collar neckband\n\
collateral confirmative confirmatory confirming corroborative corroboratory substantiating substantiative validating validatory verificatory verifying\n\
collateral indirect\n\
colleague confrere\n\
colleague workfellow\n\
collect garner gather\n\
collectable collectible\n\
collectable collectible payable\n\
collected equanimous poised\n\
collected gathered\n\
collective corporate\n\
collectivised collectivist collectivistic collectivized\n\
collectivised collectivized\n\
collectivist leftist\n\
collegial collegiate\n\
collembolan springtail\n\
collet ferrule\n\
collier pitman\n\
colloquial conversational\n\
collusive conniving\n\
collyrium eyewash\n\
cologne koln\n\
colonial compound\n\
colonised colonized settled\n\
coloniser colonizer\n\
colonist settler\n\
colony dependency\n\
coloration colouration\n\
colored colorful coloured\n\
colored coloured negro\n\
colorful colourful\n\
colorimeter tintometer\n\
colorimetric colorimetrical\n\
coloring colouring\n\
colorless colourless\n\
colors colours\n\
colossal prodigious stupendous\n\
colostrum foremilk\n\
colter coulter\n\
coltish frolicky frolicsome rollicking sportive\n\
columba dove\n\
columbarium columbary dovecote\n\
columbite niobite\n\
column pillar\n\
column pillar tower\n\
columnar columniform columnlike\n\
columnist editorialist\n\
colymbiformes podicipediformes podicipitiformes\n\
colza rape\n\
comal comate comose\n\
comate comose\n\
comb ransack\n\
combative contentious disputatious disputative litigious\n\
combativeness militance militancy\n\
combinable combinational combinatory\n\
combinative combinatorial combinatory\n\
combinative combinatory\n\
combine compound\n\
comburant comburent combustive\n\
come cum ejaculate seed semen\n\
comedian comic\n\
comestible eatable edible\n\
comestible eatable edible pabulum victual victuals\n\
cometary cometic\n\
comfort comforter quilt\n\
comfortable comfy\n\
comfortable prosperous\n\
comforter pacifier\n\
comforter sympathiser sympathizer\n\
comforting consolatory consoling\n\
comfrey cumfrey\n\
commandant commander\n\
commandeer highjack hijack pirate\n\
commanding dominating overlooking\n\
commando ranger\n\
commelinales xyridales\n\
commemorating commemorative\n\
commentator observer\n\
commentator reviewer\n\
commercialised commercialized\n\
commie communist\n\
comminatory denunciative denunciatory\n\
commissariat provender provisions viands victuals\n\
commitment committedness\n\
commodious convenient\n\
commonality commonness\n\
commonness commonplaceness everydayness\n\
commonplace humdrum prosaic unglamorous unglamourous\n\
commons park\n\
commonsense commonsensible commonsensical\n\
communicative communicatory\n\
communist communistic\n\
commutability replaceability substitutability\n\
commutability transmutability\n\
commutable substitutable\n\
compactness concentration denseness density tightness\n\
companionability companionableness\n\
comparability compare comparison equivalence\n\
comparable corresponding like\n\
comparative relative\n\
compartmental compartmentalised compartmentalized\n\
compassion pity\n\
compeer equal peer\n\
compendious succinct summary\n\
compensable paying remunerative salaried stipendiary\n\
compensated remunerated salaried stipendiary\n\
competence competency\n\
competitive competitory\n\
competitive militant\n\
competitiveness fight\n\
complainant plaintiff\n\
complaining complaintive\n\
complaisance compliance compliancy deference obligingness\n\
complaisant obliging\n\
complemental complementary completing\n\
complete concluded ended over terminated\n\
complete consummate\n\
complex complicated\n\
complexity complexness\n\
complicatedness complication knottiness tortuousness\n\
complimentary costless gratis gratuitous\n\
component constituent element\n\
composing composition\n\
composition constitution makeup\n\
compositor setter typesetter typographer\n\
comprehendible comprehensible\n\
comprehensibility understandability\n\
compress constrict contract press\n\
compressibility sponginess squeezability\n\
compressible squeezable\n\
compromising conciliatory flexible\n\
compulsive determined driven\n\
compulsiveness compulsivity\n\
compulsory mandatory required\n\
computable estimable\n\
con convict inmate yardbird\n\
conakry konakri\n\
concaveness concavity\n\
concavity incurvation incurvature\n\
concealed hidden\n\
concealment covert screen\n\
conceit conceitedness vanity\n\
conceited egotistic egotistical swollen vain\n\
conceivable imaginable\n\
conceiver mastermind originator\n\
concentrated saturated\n\
concentric concentrical homocentric\n\
conceptional ideational notional\n\
conceptive impregnable\n\
conceptus embryo\n\
concerned implicated\n\
concerned interested\n\
concerted conjunct conjunctive cooperative\n\
concessionaire concessioner\n\
conciliative conciliatory\n\
conciliator pacifier peacemaker reconciler\n\
concluding final terminal\n\
conclusiveness decisiveness finality\n\
concoction intermixture mixture\n\
concordant concurring\n\
concubine courtesan doxy paramour\n\
concupiscent lustful lusty\n\
condemnable criminal deplorable reprehensible vicious\n\
condemnatory condemning\n\
condensate condensation\n\
condescendingness condescension\n\
condescension disdainfulness superciliousness\n\
conditioned learned\n\
conditions weather\n\
condo condominium\n\
condom prophylactic rubber safe safety\n\
conducive contributing contributive contributory tributary\n\
conduction conductivity\n\
conductor director\n\
cone conoid\n\
cone strobile strobilus\n\
conelike conic conical\n\
conessi kurchee kurchi\n\
coney cony das dassie hyrax\n\
coney cony pika\n\
coney cony rabbit\n\
confectionary confectionery\n\
confederacy dixie dixieland south\n\
confident convinced\n\
confident surefooted\n\
configuration conformation contour shape\n\
confine constrain restrain\n\
confining constraining constrictive limiting restricting\n\
confirmable falsifiable verifiable\n\
confiscate forfeit forfeited\n\
conflicting contradictory\n\
confluence meeting\n\
confluent merging\n\
conformance conformity\n\
conforming conformist\n\
conformity ossification\n\
confounding contradictory\n\
confucian confucianist\n\
confucius kongfuze\n\
confusable mistakable\n\
confused disconnected disjointed disordered garbled illogical scattered unconnected\n\
confused disordered\n\
confused disoriented\n\
confusing perplexing puzzling\n\
confutable confutative questionable refutable\n\
confuter disprover rebutter refuter\n\
conge congee\n\
congealed jelled jellied\n\
congee jook\n\
congenator congener congeneric relative\n\
congeneric congenerical congenerous\n\
congeniality congenialness\n\
congenital inborn innate\n\
congested engorged\n\
congius gallon\n\
conglobation conglomeration\n\
congo congou\n\
congo zaire\n\
congratulatory gratulatory\n\
congregational congregationalist\n\
congressman congresswoman representative\n\
congruence congruity congruousness\n\
congruent congruous\n\
conidiospore conidium\n\
coniferophyta coniferophytina coniferopsida\n\
conjectural divinatory hypothetic hypothetical supposed suppositional suppositious supposititious\n\
conjoin join\n\
conjoined conjoint\n\
conjugal connubial\n\
conjugate conjugated\n\
conjugate conjugated coupled\n\
conjunction junction\n\
conjurer conjuror\n\
conjurer conjuror illusionist magician prestidigitator\n\
conk stall\n\
connatural inborn inbred\n\
connect link\n\
connecter connection connective connector connexion\n\
connecticut ct\n\
connection connexion link\n\
conodonta conodontophorida\n\
conquerable superable\n\
conqueror vanquisher\n\
conscienceless unconscionable\n\
conscientious painstaking scrupulous\n\
conscientiousness painstakingness\n\
conscious witting\n\
conscript draftee inductee\n\
consecrate consecrated dedicated\n\
consecrated sacred sanctified\n\
consecutive sequent sequential serial successive\n\
consentaneous consentient unanimous\n\
consequence effect event issue outcome result upshot\n\
consequence import moment\n\
consequential eventful\n\
conservationist environmentalist\n\
conservative conservativist\n\
conservatoire conservatory\n\
conservator curator\n\
conservatory hothouse\n\
conserve conserves preserve preserves\n\
considerateness consideration thoughtfulness\n\
consigner consignor\n\
consistence consistency\n\
consistent reproducible\n\
consistent uniform\n\
consolidative integrative\n\
consolidative unifying\n\
consonance harmoniousness\n\
consonant harmonic harmonical harmonised harmonized\n\
conspirative conspiratorial\n\
constancy stability\n\
constantan eureka\n\
constantinople istanbul stamboul stambul\n\
constituent constitutional constitutive organic\n\
constituted established\n\
constitutional inbuilt inherent integral\n\
constrained forced strained\n\
constraint restraint\n\
constricting constrictive narrowing\n\
construction structure\n\
consuming overwhelming\n\
consummate masterful masterly virtuoso\n\
consumptive lunger tubercular\n\
contact middleman\n\
contact tangency\n\
containerise containerize\n\
contaminant contamination\n\
contaminated polluted\n\
contaminating corrupting\n\
contemplativeness meditativeness pensiveness\n\
contemporaneity contemporaneousness\n\
contemporaneity contemporaneousness modernism modernity modernness\n\
contemporaneous contemporary\n\
contempt disrespect\n\
contemptuous disdainful insulting scornful\n\
content contented\n\
contentiousness quarrelsomeness\n\
conterminous contiguous\n\
contestant dissenter dissident objector protester\n\
contiguous immediate\n\
continence continency\n\
continuant fricative sibilant spirant strident\n\
continuation lengthiness prolongation protraction\n\
continue proceed\n\
continuity persistence\n\
continuous uninterrupted\n\
contort deform distort wring\n\
contorted writhed writhen\n\
contortion crookedness torsion tortuosity tortuousness\n\
contrabandist runner smuggler\n\
contrabassoon contrafagotto\n\
contraceptive preventative preventive\n\
contractor declarer\n\
contradance contredanse\n\
contrapuntal polyphonic\n\
contrariness crankiness crotchetiness grumpiness\n\
contrariness perverseness perversity\n\
contrary obstinate perverse wayward\n\
contrasting contrastive\n\
contributor subscriber\n\
contrite remorseful rueful ruthful\n\
contriver deviser planner\n\
control controller\n\
control operate\n\
control restraint\n\
controllable governable\n\
controller restrainer\n\
controversialist disputant eristic\n\
conurbation sprawl\n\
convalescent recovering\n\
convenience restroom\n\
conventicle meetinghouse\n\
convention conventionalism conventionality\n\
conventional established\n\
conventional formal schematic\n\
conventionalised conventionalized stylised stylized\n\
conversant familiar\n\
conversationalist conversationist schmoozer\n\
converse reversed transposed\n\
converter convertor\n\
convertible exchangeable\n\
convertible transformable translatable transmutable\n\
convexity convexness\n\
conveyance transport\n\
conveyer conveyor\n\
conveyer conveyor transporter\n\
convincible persuadable persuasible suasible\n\
conviviality joviality\n\
convolute convoluted\n\
convolute convolve\n\
convolution gyrus\n\
convolution swirl vortex\n\
convulse jactitate slash thresh toss\n\
convulsive spasmodic spastic\n\
cookie cooky\n\
cool coolheaded nerveless\n\
cool fine ok okay\n\
cooler tank\n\
coolie cooly\n\
coolness nervelessness\n\
coolwart foamflower\n\
coon ringtail\n\
coop hencoop henhouse\n\
coordinated interconnected unified\n\
coordinated matching\n\
coordinating coordinative\n\
copacetic copasetic copesetic copesettic\n\
copaline copalite\n\
cope coping header\n\
copeck kopeck kopek\n\
copenhagen kobenhavn\n\
copier duplicator\n\
copious voluminous\n\
copper cu\n\
coprolith faecalith fecalith stercolith\n\
copulate couple pair\n\
copyist scribe scrivener\n\
coquette flirt minx prickteaser tease vamp vamper\n\
coquettish flirtatious\n\
cora despoina kore persephone\n\
coracan corakan kurakkan ragee ragi\n\
coralberry spiceberry\n\
coralroot coralwort\n\
corbel truss\n\
cord corduroy\n\
cordate cordiform\n\
corded twilled\n\
cordial liqueur\n\
cordoba cordova\n\
cords corduroys\n\
coreopsis tickseed tickweed\n\
corgard nadolol\n\
coriaceous leathered leatherlike leathery\n\
corinth korinthos\n\
corinthian playboy\n\
corium derma dermis\n\
cork phellem\n\
corked corky\n\
cormose cormous\n\
corn maize\n\
corned cured\n\
cornel dogwood\n\
corneous hornlike horny\n\
corner niche recess recession\n\
corner nook\n\
corner quoin\n\
corner tree\n\
cornered trapped treed\n\
cornet horn trump trumpet\n\
cornetist trumpeter\n\
cornflour cornstarch\n\
cornflower strawflower\n\
cornhusker nebraskan\n\
cornice pelmet valance\n\
cornpone pone\n\
cornucopia profuseness profusion richness\n\
corona corposant\n\
coroneted highborn titled\n\
corporality corporeality materiality physicalness\n\
corporate incorporated\n\
corporation potbelly tummy\n\
corporeal material\n\
corpulency fleshiness obesity\n\
corpulent obese rotund weighty\n\
correct decline slump\n\
correctitude properness propriety\n\
corrective disciplinal disciplinary\n\
corrective restorative\n\
correctness rightness\n\
correlate correlated correlative\n\
correspondence parallelism\n\
correspondence symmetricalness symmetry\n\
correspondent newspaperman newspaperwoman newswriter pressman\n\
corrodentia psocoptera\n\
corrupt corrupted\n\
corrupt crooked\n\
corrupt tainted\n\
corrupted debased vitiated\n\
corrupting degrading\n\
corruption corruptness\n\
corruption degeneracy depravation depravity putrefaction\n\
corruptive perversive pestiferous\n\
corse corsica\n\
corselet corslet\n\
corset girdle stays\n\
cortef cortisol hydrocortisone hydrocortone\n\
cortes cortez\n\
cortex pallium\n\
corticifugal corticoefferent corticofugal\n\
corticipetal corticoafferent\n\
corticoid corticosteroid\n\
corundom corundum\n\
corvus crow\n\
corydalis corydalus\n\
corythosaur corythosaurus\n\
cos romaine\n\
coseismal coseismic\n\
cosher kosher\n\
cosignatory cosigner\n\
cosmea cosmos\n\
cosmetic decorative ornamental\n\
cosmetic enhancive\n\
cosmogenic cosmogonic cosmogonical cosmologic cosmological\n\
cosmographer cosmographist\n\
cosmologic cosmological\n\
cosmopolitan cosmopolite\n\
cosmopolitan ecumenical general oecumenical universal worldwide\n\
cosmos creation existence macrocosm universe world\n\
coss kos\n\
cost price\n\
cost price toll\n\
costa rib\n\
costate ribbed\n\
costliness dearness preciousness\n\
costly pricey pricy\n\
costumer costumier\n\
cosy cozy\n\
cosy cozy snug\n\
cot crib\n\
cot fingerstall\n\
cothromboplastin proconvertin\n\
cottar cotter\n\
cotter cottier\n\
couch lounge sofa\n\
coumadin warfarin\n\
coumarouna dipteryx\n\
counsellor counselor\n\
countable denumerable enumerable numerable\n\
countenance kisser mug phiz physiognomy smiler visage\n\
countenance visage\n\
counter tabulator\n\
counterbalance counterpoise counterweight equaliser equalizer\n\
counterbalance equilibrium equipoise\n\
counterbalanced counterpoised\n\
counterbore countersink\n\
counterfeit forgery\n\
counterfeit imitative\n\
counterfeiter forger\n\
counterglow gegenschein\n\
counterman counterperson counterwoman\n\
counterpart similitude twin\n\
counterrevolutionary counterrevolutionist\n\
counterspy mole\n\
counterterror counterterrorist\n\
countless infinite innumerable innumerous multitudinous myriad numberless uncounted unnumberable unnumbered unnumerable\n\
countlessness innumerableness\n\
countrified countryfied rustic\n\
country state\n\
countryman ruralist\n\
countrywide nationwide\n\
couple pair twin\n\
coupled joined linked\n\
coupler coupling\n\
coupling yoke\n\
courgette zucchini\n\
courier messenger\n\
course feed flow\n\
course path\n\
course row\n\
course trend\n\
court courtroom\n\
court courtyard\n\
court lawcourt\n\
courteous gracious nice\n\
courtly formal stately\n\
couthie couthy\n\
couturier designer\n\
covalence covalency\n\
covetous envious jealous\n\
coville hediondilla\n\
cowardice cowardliness\n\
cowardly fearful\n\
cowberry foxberry lingberry lingenberry lingonberry\n\
cowberry lingonberry\n\
cowcatcher fender pilot\n\
cower crawl creep cringe fawn grovel\n\
cower huddle\n\
cowhide cowskin\n\
cowrie cowry\n\
cowslip kingcup\n\
cowslip paigle\n\
cox coxswain\n\
cox cyclooxygenase\n\
coxa hip\n\
coy demure overmodest\n\
coyness demureness\n\
coypu nutria\n\
cozy informal\n\
cpu mainframe processor\n\
crab crabmeat\n\
crabbed crabby fussy grouchy grumpy\n\
crabbedness crabbiness crossness\n\
crabwise sideways\n\
crackbrained idiotic\n\
cracker redneck\n\
cracker snapper\n\
crackerjack jimdandy jimhickey\n\
crackle crackleware\n\
crackling greaves\n\
crackpot fruitcake nut screwball\n\
cracksman safebreaker safecracker\n\
cracow krakau krakow\n\
cradle rocker\n\
crafter craftsman\n\
craftiness deceitfulness guile\n\
crafty cunning dodgy foxy guileful knavish sly tricksy tricky wily\n\
cragged craggy hilly mountainous\n\
cram jam jampack wad\n\
crampfish numbfish torpedo\n\
crampon crampoon\n\
cranch craunch crunch grind\n\
crane grus\n\
craniata vertebrata\n\
craniate vertebrate\n\
craniologist phrenologist\n\
craniometric craniometrical\n\
cranky fractious irritable nettlesome peckish peevish pettish petulant scratchy techy testy tetchy\n\
cranky tippy\n\
crap dirt poop shit shite turd\n\
crape crepe\n\
crape crimp frizz frizzle kink\n\
crappy icky lousy rotten shitty stinking stinky\n\
crapulent crapulous\n\
crash dash\n\
crasher gatecrasher\n\
crassitude crassness\n\
crate crateful\n\
craved desired\n\
craven poltroon recreant\n\
craven recreant\n\
crawdad crawdaddy crawfish crayfish\n\
crawdad crawfish crayfish ecrevisse\n\
crawfish crayfish langouste\n\
crawl creep\n\
crawler creeper\n\
crawler lackey sycophant toady\n\
crayfish langouste\n\
crazed deranged\n\
crazy dotty gaga\n\
crazy looney loony nutcase weirdo\n\
crazy screwball softheaded\n\
crazyweed locoweed\n\
creaky decrepit derelict woebegone\n\
creaky screaky\n\
cream emollient ointment\n\
cream skim\n\
creaminess soupiness\n\
creaseless uncreased\n\
creaseproof wrinkleproof\n\
creashak mealberry sandberry\n\
creatin creatine\n\
creative originative\n\
creature puppet tool\n\
creature wight\n\
credal creedal\n\
credence credenza\n\
creditworthy responsible\n\
credulousness gullibility\n\
creep mouse pussyfoot sneak\n\
creep spook weirdie weirdo weirdy\n\
creese kris\n\
crematorium crematory\n\
crenate crenated scalloped\n\
crenation crenature crenel crenelle scallop\n\
crenel crenelle\n\
crenulate crenulated\n\
crescent lunate semilunar\n\
cresson watercress\n\
crest summit\n\
crested plumed\n\
crested topknotted tufted\n\
crete kriti\n\
crewet cruet\n\
crewman sailor\n\
crier weeper\n\
criminal crook felon malefactor outlaw\n\
criminal felonious\n\
criminative criminatory incriminating incriminatory\n\
crimp crimper\n\
crimp flexure fold plication\n\
crimp pinch\n\
crimper curler roller\n\
crimson flushed reddened\n\
crimson ruby\n\
crimson violent\n\
cringe flinch funk quail recoil shrink squinch wince\n\
cringing groveling grovelling wormlike wormy\n\
cringle eyelet grommet grummet loop\n\
crinion trichion\n\
crinkle furrow line seam wrinkle\n\
crinkle ruckle scrunch wrinkle\n\
crinkled crinkly rippled wavelike wavy\n\
crinkleroot toothwort\n\
crinoline hoopskirt\n\
crippled game gimpy halt halting lame\n\
crippling disabling incapacitating\n\
crisscross crisscrossed\n\
criterial criterional\n\
critical decisive\n\
critical vital\n\
crixivan indinavir\n\
cro oscilloscope scope\n\
croaky guttural\n\
croat croatian\n\
croatia hrvatska\n\
crochet crocheting\n\
crock lampblack smut soot\n\
crockery dishware\n\
crocodilia crocodylia\n\
crocodilus crocodylus\n\
cromlech dolmen\n\
cromorne crumhorn krummhorn\n\
cromwell ironsides\n\
crook turn\n\
crookback crookbacked gibbous humpbacked humped hunchbacked kyphotic\n\
crookback humpback hunchback\n\
crooked hunched stooped stooping\n\
crookedness deviousness\n\
cropper sharecropper\n\
crossbeam crosspiece trave traverse\n\
crossbreed hybrid\n\
crossbreed hybridise hybridize interbreed\n\
crosscut cutoff shortcut\n\
crosshatch hachure hatch hatching\n\
crosshatched hatched\n\
crossing crossover crosswalk\n\
crossing ford\n\
crossopterygian lobefin\n\
crosstie sleeper\n\
crotal crottal crottle\n\
crotalaria rattlebox\n\
crotch fork\n\
crotch genitalia genitals privates\n\
crotchet hook\n\
crotchet oddity queerness quirk quirkiness\n\
crouch hunker scrunch squat\n\
croup croupe hindquarters rump\n\
crowbar pry\n\
crowd herd\n\
crowned laureled laurelled\n\
crownless uncrowned\n\
crownwork jacket\n\
crucial essential\n\
crucial important\n\
cruciate cruciform\n\
crucifix rood\n\
crud filth skank\n\
cruddy filthy nasty smutty\n\
crudeness crudity gaucheness\n\
crudeness roughness\n\
cruelness cruelty harshness\n\
cruller twister\n\
crumbliness friability\n\
crumbly friable\n\
crusader meliorist reformer reformist\n\
crush jam\n\
crush mash squash squelch\n\
crushed humbled humiliated\n\
crushing devastating\n\
crust encrustation incrustation\n\
crustacean crustaceous\n\
crusted crustlike crusty encrusted\n\
crusty curmudgeonly gruff\n\
crying egregious flagrant glaring rank\n\
cryptanalyst cryptographer cryptologist\n\
cryptanalytic cryptographic cryptographical cryptologic cryptological\n\
cryptic cryptical inscrutable mysterious mystifying\n\
cryptogamic cryptogamous\n\
cryptomonad cryptophyte\n\
crystal crystallization\n\
crystal lechatelierite quartz\n\
crystalised crystallized\n\
crystalline limpid lucid pellucid transparent\n\
crystallisation crystallization crystallizing\n\
crystallised crystallized\n\
cub greenhorn rookie\n\
cub lad laddie sonny\n\
cubby cubbyhole snug snuggery\n\
cubbyhole pigeonhole\n\
cube dice\n\
cubelike cubical cubiform cuboid cuboidal\n\
cubist cubistic\n\
cubitus elbow\n\
cucumber cuke\n\
cuddle nest nestle nuzzle snuggle\n\
cuddlesome cuddly\n\
cudgel fustigate\n\
cudweed filago\n\
cuff handcuff handlock manacle\n\
cuff handcuff manacle\n\
cuff turnup\n\
cuff whomp\n\
culprit perpetrator\n\
cultivator tiller\n\
cultural ethnic ethnical\n\
cumber encumber restrain\n\
cumbersome cumbrous\n\
cumquat kumquat\n\
cumulonimbus thundercloud\n\
cunctator postponer procrastinator\n\
cuneal cuneiform\n\
cunning cute\n\
cunt puss pussy slit snatch twat\n\
cuon cyon\n\
cup cupful\n\
cupflower nierembergia\n\
cuppa cupper\n\
cupric cuprous\n\
cuprimine penicillamine\n\
cupular cupulate\n\
cuquenan kukenaam\n\
cur mongrel mutt\n\
curability curableness\n\
curacao curacoa\n\
curare tubocurarine\n\
curate minister parson pastor rector\n\
curative cure remedy therapeutic\n\
curb curbing kerb\n\
curbstone kerbstone\n\
cured healed recovered\n\
cured vulcanised vulcanized\n\
curet curette\n\
curio curiosity oddity oddment peculiarity rarity\n\
curious funny odd peculiar queer rum rummy singular\n\
curiousness foreignness strangeness\n\
curl lock ringlet whorl\n\
curl wave\n\
curled curling\n\
curliness waviness\n\
currajong kurrajong\n\
currency currentness\n\
cursed curst\n\
cursed damned doomed unredeemed unsaved\n\
cursor pointer\n\
cursory passing perfunctory superficial\n\
curt laconic terse\n\
curtain drape drapery pall\n\
curtainless uncurtained\n\
curtilage grounds yard\n\
curtsey curtsy\n\
curvaceousness shapeliness voluptuousness\n\
curved curving\n\
curvey curvy\n\
curvilineal curvilinear\n\
cusco cuzco\n\
cushat ringdove\n\
cushion shock\n\
cushion soften\n\
cushioned cushiony padded\n\
cushioning padding\n\
cushy easygoing\n\
cusk torsk\n\
cusp leaflet\n\
cuspate cuspated cusped cuspidal cuspidate cuspidated\n\
cuspidor spittoon\n\
cussed obdurate obstinate unrepentant\n\
cussedness orneriness\n\
custodial tutelar tutelary\n\
custodian keeper steward\n\
customhouse customshouse\n\
cutaneal cutaneous dermal\n\
cutch kutch\n\
cute precious\n\
cuteness prettiness\n\
cuticle epidermis\n\
cuticular dermal epidermal epidermic\n\
cutis tegument\n\
cutlas cutlass\n\
cutlassfish hairtail\n\
cutlery cutter\n\
cutlet escallop scallop scollop\n\
cutpurse pickpocket\n\
cutter pinnace\n\
cutter stonecutter\n\
cutting edged stinging\n\
cutting keen knifelike lancinate lancinating piercing stabbing\n\
cuttle cuttlefish\n\
cwt hundredweight\n\
cyan teal\n\
cyanamid cyanamide\n\
cyanide nitril nitrile\n\
cyanite kyanite\n\
cyanobacterial cyanophyte\n\
cyanogenetic cyanogenic\n\
cyanuramide melamine\n\
cybele dindymene\n\
cyberspace internet net\n\
cycadofilicales lyginopteridales\n\
cycadophyta cycadophytina cycadopsida\n\
cyclades kikladhes\n\
cycle motorbike motorcycle\n\
cyclic cyclical\n\
cyclicity periodicity\n\
cyclobenzaprine flexeril\n\
cycloid cycloidal\n\
cyclonal cyclonic cyclonical\n\
cyclorama diorama panorama\n\
cyclosis streaming\n\
cydippea cydippida cydippidea\n\
cylindric cylindrical\n\
cylindricality cylindricalness\n\
cylix kylix\n\
cyma cymatium\n\
cymbid cymbidium\n\
cymograph kymograph\n\
cynewulf cynwulf\n\
cynic faultfinder\n\
cynical misanthropic misanthropical\n\
cypre princewood salmwood\n\
cyprian cypriot cypriote\n\
cyprinid cyprinoid\n\
cyproheptadine periactin\n\
cyrilla leatherwood\n\
cyst vesicle\n\
cytidine deoxycytidine\n\
cytoarchitectonic cytoarchitectural\n\
cytoarchitectonics cytoarchitecture\n\
cytogenetic cytogenetical\n\
cytokinin kinin\n\
cytol cytoplasm\n\
cytologic cytological\n\
cytoplasmatic cytoplasmic\n\
cytosmear smear\n\
czar tsar tzar\n\
czarina czaritza tsarina tsaritsa tzarina\n\
czarist czaristic tsarist tsaristic tzarist\n\
czech czechoslovak czechoslovakian\n\
czech czechoslovakian\n\
dab pat\n\
dab splash splatter\n\
dab swab swob\n\
dabbled spattered splashed splattered\n\
dabbler dilettante sciolist\n\
dacca dhaka\n\
dachshund dachsie\n\
dacoit dakoit\n\
dacron terylene\n\
dactyl digit\n\
dad dada daddy pa papa pappa pop\n\
dado wainscot\n\
daedal daedalus\n\
daemon daimon demon devil fiend\n\
daemon demigod\n\
dag decagram dekagram dkg\n\
dag jag\n\
dagger sticker\n\
dago ginzo greaseball guinea wop\n\
dahl dhal\n\
daikon radish\n\
daily everyday\n\
daintiness fineness\n\
dainty exquisite\n\
dainty goody kickshaw treat\n\
dainty nice overnice prissy squeamish\n\
dairen dalian talien\n\
dairymaid milkmaid\n\
daishiki dashiki\n\
dak dhak palas\n\
dal decaliter decalitre dekaliter dekalitre dkl\n\
dallier dillydallier lounger mope\n\
dallisgrass paspalum\n\
dally dawdle\n\
dalmane flurazepam\n\
dalo dasheen taro\n\
dam decameter decametre dekameter dekametre dkm\n\
dam dike dyke\n\
damaged discredited\n\
damaging detrimental prejudicial prejudicious\n\
damaging negative\n\
damar dammar\n\
damascus dimash\n\
dame gentlewoman lady madam\n\
damgalnunna damkina\n\
damn darn hoot shit shucks\n\
damn goddamn\n\
damnable execrable\n\
damnatory damning\n\
damoiselle damosel damozel damsel demoiselle\n\
damp dampish moist\n\
dampener moistener\n\
damper muffler\n\
damselfish demoiselle\n\
dana danu\n\
danau danube\n\
dancer terpsichorean\n\
dandified dandyish foppish\n\
dandy yawl\n\
dandyism foppishness\n\
dangerous grievous serious\n\
dangerous unsafe\n\
danmark denmark\n\
dantean dantesque\n\
danzig gdansk\n\
dapper dashing jaunty natty raffish rakish spiffy spruce\n\
dapperness jauntiness nattiness rakishness\n\
dapple fleck maculation speckle\n\
dappled mottled\n\
dapsang k2\n\
dardan dardanian trojan\n\
dardanelles hellespont\n\
daredevil hothead lunatic madcap swashbuckler\n\
daredevil temerarious\n\
daredevilry daredeviltry\n\
daricon oxyphencyclimine\n\
darkness duskiness swarthiness\n\
darkness shadow\n\
darling dearie deary ducky favorite favourite pet\n\
darmera peltiphyllum\n\
darmstadtium ds\n\
darn mend\n\
dart dash flash scoot scud shoot\n\
dart fleet flit flutter\n\
darvon propoxyphene\n\
dash elan flair panache style\n\
dashboard fascia\n\
dashboard splashboard splasher\n\
dashed dotted\n\
dastard dastardly\n\
datable dateable\n\
date escort\n\
dateless endless sempiternal\n\
dateless timeless\n\
dateless undated\n\
daub plaster\n\
daub smear\n\
daughter girl\n\
daunting intimidating\n\
dauntlessness intrepidity\n\
davis davys\n\
daw jackdaw\n\
dawdle lag\n\
dawdle linger\n\
dawdler drone laggard lagger trailer\n\
daybook ledger\n\
daydreamer woolgatherer\n\
dayflower spiderwort\n\
dayfly mayfly shadfly\n\
daypro oxaprozin\n\
daystar lucifer phosphorus\n\
dazed foggy groggy logy stuporous\n\
dazed stunned stupefied stupid\n\
dazzling fulgurant fulgurous\n\
db decibel\n\
db dubnium hahnium\n\
ddc dideoxycytosine zalcitabine\n\
ddi didanosine dideoxyinosine\n\
ddt dichlorodiphenyltrichloroethane\n\
de delaware\n\
deadbeat defaulter\n\
deaden girdle\n\
deadliness lethality\n\
deadlocked stalemated\n\
deadness unresponsiveness\n\
deadpan expressionless impassive unexpressive\n\
deafening earsplitting thunderous thundery\n\
deal softwood\n\
dealer principal\n\
dean doyen\n\
dearth paucity\n\
deathless undying\n\
deathlike deathly\n\
deathly mortal\n\
deb debutante\n\
debark disembark\n\
debased degraded devalued\n\
debaser degrader\n\
debasing degrading\n\
debatable disputable\n\
debatable problematic problematical\n\
debauched degenerate degraded dissipated dissolute fast libertine profligate riotous\n\
debauchee libertine rounder\n\
debaucher ravisher violator\n\
debile decrepit feeble infirm rickety sapless weakly\n\
debilitative enervating enfeebling weakening\n\
debitor debtor\n\
debonair debonaire debonnaire suave\n\
debris detritus dust junk rubble\n\
dec declination\n\
decadron dexamethasone dexone hexadrol oradexon\n\
decal decalcomania\n\
decamp skip vamoose\n\
decant pour\n\
decay decomposition\n\
decayable putrefiable putrescible spoilable\n\
decayed rotted rotten\n\
deceased decedent departed\n\
deceit fraudulence\n\
deceitful fallacious fraudulent\n\
deceleration retardation slowing\n\
decent fitting\n\
decent nice\n\
decentralised decentralized\n\
decentralising decentralizing\n\
deceptive delusory\n\
deceptive misleading\n\
deceptiveness obliquity\n\
decided distinct\n\
deciding determinant determinative determining\n\
decigram dg\n\
deciliter decilitre dl\n\
decimal denary\n\
decimeter decimetre dm\n\
decipherable readable\n\
decipherer decoder\n\
decision decisiveness\n\
decker dekker\n\
deckhand roustabout\n\
deckled featheredged\n\
declarative indicative\n\
declared stated\n\
declension declination decline declivity descent downslope fall\n\
declivitous downhill\n\
decompress uncompress\n\
decoration ornament ornamentation\n\
decorator ornamentalist\n\
decorousness decorum\n\
decouple uncouple\n\
decoy steerer\n\
decrease decrement\n\
decreased reduced\n\
decrescendo diminuendo\n\
decussate intersectant intersecting\n\
deedbox strongbox\n\
deepening thickening\n\
deepfreeze freezer\n\
deepness depth\n\
deepness profoundness profundity\n\
defalcator embezzler peculator\n\
defeated disappointed discomfited foiled frustrated thwarted\n\
defeatist negativist\n\
defecator shitter voider\n\
defect shortcoming\n\
defective faulty\n\
defector deserter\n\
defence defense\n\
defenceless defenseless\n\
defencelessness defenselessness unprotectedness\n\
defendable defensible\n\
defendant suspect\n\
defender guardian protector shielder\n\
defender withstander\n\
defenseless naked\n\
defensive justificative justificatory\n\
deference respect respectfulness\n\
deferent deferential regardful\n\
defiance rebelliousness\n\
defiant noncompliant\n\
deficiency inadequacy insufficiency\n\
deficient inferior substandard\n\
deficient insufficient\n\
deficient lacking wanting\n\
deficit shortage shortfall\n\
defile gorge\n\
defile maculate stain sully tarnish\n\
defiled maculate\n\
defiler polluter\n\
defined outlined\n\
definiteness determinateness\n\
definitive determinate\n\
definitive unequivocal\n\
deflective refractive\n\
deflower ruin\n\
defoliate defoliated\n\
deform flex turn\n\
deformed distorted malformed misshapen\n\
deformity disfiguration disfigurement\n\
defroster deicer\n\
deft dexterous dextrous\n\
degage uninvolved\n\
degenerate deviant deviate pervert\n\
deglycerolise deglycerolize\n\
degree grade\n\
dehumanised dehumanized unhuman\n\
dehydrated desiccated dried\n\
deist deistic\n\
deity divinity god immortal\n\
dejeuner lunch luncheon tiffin\n\
delavirdine rescriptor\n\
delawarean delawarian\n\
delectability deliciousness lusciousness toothsomeness\n\
delectable delicious luscious scrumptious toothsome yummy\n\
deleterious hurtful injurious\n\
deli delicatessen\n\
deliberate intentional knowing\n\
deliberateness deliberation\n\
deliberateness deliberation slowness unhurriedness\n\
delicate finespun\n\
delicate fragile frail\n\
delicate ticklish touchy\n\
delicious delightful\n\
delilah enchantress siren temptress\n\
delineate delineated represented\n\
delineate describe line\n\
delineation depiction limning\n\
delineative depictive\n\
delinquency dereliction\n\
delinquent derelict neglectful remiss\n\
delinquent overdue\n\
delirious excited frantic mad unrestrained\n\
delirious hallucinating\n\
deliverer deliveryman\n\
deliverer rescuer savior saviour\n\
dell dingle\n\
delphian delphic\n\
delphic oracular\n\
deltasone meticorten orasone prednisone\n\
deluge flood inundate swamp\n\
deluge flood inundation torrent\n\
deluxe gilded luxurious opulent princely sumptuous\n\
deluxe luxe\n\
delve dig\n\
demagog demagogue\n\
demagogic demagogical\n\
demarcation limit\n\
demeaning humbling humiliating mortifying\n\
demerit fault\n\
demerol meperidine\n\
demesne domain\n\
demigod superman ubermensch\n\
democratic popular\n\
demodulator detector\n\
demographer demographist\n\
demolished dismantled razed\n\
demon devil fiend monster ogre\n\
demoniac demoniacal possessed\n\
demonic diabolic diabolical fiendish hellish infernal satanic unholy\n\
demonstrability provability\n\
demonstrable incontrovertible\n\
demonstrable provable\n\
demonstrative illustrative\n\
demonstrator protester\n\
demoralised demoralized discouraged disheartened\n\
demoralising demoralizing disheartening dispiriting\n\
demulcent emollient salving softening\n\
demythologised demythologized\n\
den hideaway hideout\n\
den lair\n\
denali mckinley\n\
denary tenfold\n\
denatured denaturised denaturized\n\
dendraspis dendroaspis\n\
denim dungaree jean\n\
denim jeans\n\
denizen dweller habitant indweller inhabitant\n\
denotative denotive\n\
denotative explicit\n\
dense dumb obtuse slow\n\
dense impenetrable\n\
denseness density\n\
densimeter densitometer\n\
dent ding gouge nick\n\
dent incision slit\n\
dent indent\n\
dentin dentine\n\
dentition teeth\n\
denture plate\n\
deodorant deodourant\n\
deoxyguanosine guanosine\n\
deoxythymidine thymidine\n\
depart digress sidetrack straggle\n\
depart quit\n\
departer goer leaver\n\
dependability dependableness reliability reliableness\n\
dependable honest reliable\n\
dependable reliable\n\
dependable safe\n\
dependant hooked\n\
dependant qualified\n\
depicted pictured portrayed\n\
depilator depilatory epilator\n\
deplorable distressing lamentable pitiful sad sorry\n\
deplorable execrable woeful\n\
deplumate deplume displume\n\
depolarisation depolarization\n\
deponent deposer testifier\n\
deportee exile\n\
depositary depository repository\n\
depot entrepot storage store storehouse\n\
depot terminal terminus\n\
depraved perverse perverted reprobate\n\
depreciating depreciative depreciatory\n\
depreciator detractor disparager knocker\n\
depress lower\n\
depressant downer sedative\n\
depressed dispirited downcast downhearted gloomy\n\
depression impression imprint\n\
deprivation loss\n\
deprived disadvantaged\n\
deputy lieutenant\n\
deputy surrogate\n\
deracinate extirpate uproot\n\
derisive gibelike jeering mocking taunting\n\
dermal dermic\n\
dermatologic dermatological\n\
derogative derogatory disparaging\n\
des diethylstilbesterol stilbesterol\n\
des diethylstilbestrol diethylstilboestrol stilbestrol stilboestrol\n\
descale scale\n\
descend fall\n\
descendant descendent\n\
descent extraction origin\n\
desegrated nonsegregated unsegregated\n\
desensitising desensitizing\n\
deserved merited\n\
deserving worth\n\
deservingness merit meritoriousness\n\
desexualise desexualize\n\
desiccant drier siccative\n\
design pattern\n\
designed intentional unintentional\n\
designer intriguer\n\
designing scheming\n\
desirability desirableness\n\
desirability desirableness oomph\n\
desirable suitable worthy\n\
desirous wishful\n\
despairing desperate\n\
despatch dispatch\n\
despatch dispatch expedition expeditiousness\n\
desperate dire\n\
desperate heroic\n\
despicable slimy ugly unworthy vile worthless\n\
despised detested hated scorned\n\
despiteful spiteful vindictive\n\
despoil plunder rape spoil violate\n\
despoiled pillaged raped ravaged sacked\n\
despoiler freebooter looter pillager plunderer raider spoiler\n\
despondent heartsick\n\
despotic despotical\n\
desquamation peeling shedding\n\
dessertspoon dessertspoonful\n\
destination finish goal\n\
destiny fate\n\
destitute impoverished indigent necessitous needy\n\
destroy ruin\n\
destroyed ruined\n\
destroyer ruiner undoer uprooter waster\n\
desyrel trazodone\n\
detailed elaborate elaborated\n\
detectable noticeable\n\
detectable perceptible\n\
detective investigator tec\n\
detector sensor\n\
detergence detergency\n\
detergent detersive\n\
determinant epitope\n\
determination purpose\n\
determined dictated\n\
determinist fatalist predestinarian predestinationist\n\
detroit motown\n\
deuce ii two\n\
deuce two\n\
deuteromycota deuteromycotina\n\
deutschland frg germany\n\
developing underdeveloped\n\
deviate divert\n\
devil heller hellion\n\
devilfish manta\n\
devilfish octopus\n\
devilish diabolic diabolical mephistophelean mephistophelian\n\
devilish rascally roguish\n\
devious oblique\n\
devious shifty\n\
deviousness obliqueness\n\
devon devonshire\n\
devotee fan lover\n\
devout earnest heartfelt\n\
devoutness religiousness\n\
dextroglucose dextrose\n\
dextrorotary dextrorotatory\n\
dextrorsal dextrorse\n\
dhava dhawa\n\
dhodhekanisos dodecanese\n\
diabeta glyburide micronase\n\
diabolist satanist\n\
diacetylmorphine heroin\n\
diachronic historical\n\
diacritic diacritical\n\
diaglyph intaglio\n\
diagnostic symptomatic\n\
diagnostician pathologist\n\
diagrammatic diagrammatical\n\
dialectic dialectical\n\
diam diameter\n\
diamante sequin spangle\n\
diametral diametric diametrical\n\
diametric diametrical opposite polar\n\
diamond infield\n\
diamond rhomb rhombus\n\
dianoetic discursive\n\
diaper napkin nappy\n\
diaphoretic sudorific\n\
diaphragm midriff\n\
diaphragm pessary\n\
diaphyseal diaphysial\n\
diarist journalist\n\
diarrheal diarrheic diarrhetic diarrhoeal diarrhoeic diarrhoetic\n\
dias diaz\n\
diatomite kieselguhr\n\
diazepam valium\n\
diazoxide hyperstat\n\
dibber dibble\n\
dibbuk dybbuk\n\
dibranch dibranchiate\n\
dibranchia dibranchiata\n\
dicamptodon dicamptodontid\n\
dice die\n\
dick gumshoe hawkshaw\n\
dick pecker putz tool\n\
dickey dickie dicky\n\
dickey dickie dicky shirtfront\n\
dickey dicky\n\
dickeybird dickybird\n\
dicloxacillin dynapen\n\
dicot dicotyledon exogen magnoliopsid\n\
dicotyledonae dicotyledones magnoliopsida\n\
dicoumarol dicumarol\n\
dictator potentate\n\
didactic didactical\n\
diddle fiddle play toy\n\
diddley diddly diddlyshit diddlysquat shit squat\n\
didrikson zaharias\n\
diehard traditionalist\n\
dielectric insulator nonconductor\n\
diemaker diesinker\n\
diestrous diestrual dioestrous dioestrual\n\
dietary dietetic dietetical\n\
dietician dietitian\n\
dietrich thiry\n\
difference remainder\n\
differentiator discriminator\n\
difficult unmanageable\n\
difficultness difficulty\n\
diffident shy timid unsure\n\
diffuse diffused\n\
diffuse imbue interpenetrate penetrate permeate pervade riddle\n\
diffuser diffusor\n\
diffusing diffusive dispersive disseminative\n\
diffusion dissemination\n\
diflunisal dolobid\n\
dig excavate hollow\n\
dig excavation\n\
dig jab prod stab\n\
digenesis metagenesis\n\
digestibility digestibleness\n\
digger excavator shovel\n\
diggings digs\n\
diggings digs domiciliation lodgings\n\
digit finger fingerbreadth\n\
digitalin digitalis\n\
digitalis foxglove\n\
digitate fingerlike\n\
digitiser digitizer\n\
dignifying ennobling\n\
dignitary panjandrum vip\n\
dignity gravitas lordliness\n\
digoxin lanoxin\n\
digressive discursive excursive rambling\n\
digressive tangential\n\
dihydroxyphenylalanine dopa\n\
dike dyke\n\
dilantin diphenylhydantoin phenytoin\n\
dilater dilator\n\
dilatoriness procrastination\n\
dilatory laggard pokey poky\n\
dilaudid hydromorphone\n\
dilettante dilettanteish dilettantish sciolistic\n\
diligence industriousness industry\n\
diligent persevering\n\
diluent dilutant thinner\n\
dilute diluted\n\
diluvial diluvian\n\
dimenhydrinate dramamine\n\
dimension proportion\n\
diminished lessened vitiated weakened\n\
diminutiveness minuteness petiteness tininess weeness\n\
dimness faintness\n\
dimness subduedness\n\
dimorphic dimorphous\n\
dimwit doofus nitwit\n\
dinghy dory rowboat\n\
dingo warragal warrigal\n\
dingy disconsolate dismal drab drear dreary gloomy sorry\n\
dingy muddied muddy\n\
dinkey dinky\n\
dinky insignificant\n\
dinoceras uintathere\n\
dioecian dioecious\n\
diol glycol\n\
diopter dioptre\n\
diovan valsartan\n\
dipladenia mandevilla\n\
diplomacy discreetness finesse\n\
diplomacy statecraft statesmanship\n\
diplomat diplomatist\n\
diplomatic diplomatical\n\
diplopoda myriapoda\n\
dipped lordotic swayback swaybacked\n\
dipper plough wagon wain\n\
dipteran dipteron\n\
dire direful dread dreaded dreadful fearful fearsome frightening horrendous horrific terrible\n\
directing directional directive guiding\n\
direction way\n\
directionality directivity\n\
directiveness directivity\n\
directness straightness\n\
director manager\n\
dirigible steerable\n\
dirt soil\n\
dirt ungraded\n\
dirtiness smuttiness\n\
dis orcus pluto\n\
disabled handicapped\n\
disabling disqualifying\n\
disabused undeceived\n\
disadvantageous unfavorable unfavourable\n\
disaffected malcontent rebellious\n\
disagreeable unsympathetic\n\
disagreement discrepancy divergence variance\n\
disappointing dissatisfactory unsatisfying\n\
disarmer pacificist pacifist\n\
disarray disorderliness\n\
disarticulate disjoint\n\
disband dissolve\n\
disbelieving sceptical skeptical unbelieving\n\
disburden unburden\n\
disburser expender spender\n\
disc disk\n\
disc disk platter record\n\
disc disk saucer\n\
discalceate discalced unshod\n\
discarded throwaway\n\
discernability legibility\n\
discernable discernible\n\
discernible evident observable\n\
discerning discreet\n\
discernment discretion\n\
discerp dismember\n\
discerp lop sever\n\
discharge emission\n\
discharge unload\n\
discharged dismissed fired\n\
disciplinarian martinet moralist\n\
disclike discoid discoidal disklike\n\
disco discotheque\n\
discoloration discolouration stain\n\
discombobulated disconcerted\n\
disconcerting upsetting\n\
disconfirming invalidating\n\
disconfirming negative\n\
disconnect disconnection gulf\n\
disconnect unplug\n\
disconnected disunited fragmented split\n\
disconnected staccato\n\
disconsolate inconsolable unconsolable\n\
discontent discontented\n\
discontinuous noncontinuous\n\
discord discordance\n\
discordant disharmonious dissonant inharmonic\n\
discourteous ungracious\n\
discourtesy rudeness\n\
discoverer finder spotter\n\
discredited disgraced dishonored shamed\n\
discrepant dissonant\n\
discrepant inconsistent\n\
discrete distinct\n\
discretional discretionary\n\
discriminative discriminatory\n\
discriminative judicial\n\
discriminatory invidious\n\
discriminatory preferential\n\
discriminatory prejudiced\n\
discus saucer\n\
disdainful haughty imperious lordly overbearing prideful sniffy supercilious swaggering\n\
diseased morbid pathologic pathological\n\
disenchanting disillusioning\n\
disencumber disentangle extricate untangle\n\
disenfranchised disfranchised voiceless voteless\n\
disengage withdraw\n\
disentangle unsnarl\n\
disentangle unwind\n\
disentangled loosened unsnarled\n\
disentangler unraveler unraveller\n\
disgorge shed spill\n\
disgraceful ignominious inglorious opprobrious shameful\n\
disgraceful scandalous shameful shocking\n\
disgracefulness ignominiousness shamefulness\n\
disgruntled dissatisfied\n\
disgustingness distastefulness nauseatingness sickeningness unsavoriness\n\
disgustingness unsavoriness\n\
dish dishful\n\
dish saucer\n\
disharmony inharmoniousness\n\
dishcloth dishrag\n\
dished patelliform\n\
dishevel tangle tousle\n\
disheveled dishevelled frowzled rumpled tousled\n\
dishonest dishonorable\n\
dishonor dishonour\n\
dishonorable dishonourable\n\
dishonorableness dishonourableness\n\
disinclination hesitancy hesitation indisposition reluctance\n\
disintegrable meltable\n\
disjoin disjoint\n\
disjointed dislocated separated\n\
diskette floppy\n\
dislodge reposition\n\
dislogistic dyslogistic pejorative\n\
disloyal unpatriotic\n\
dismount unhorse\n\
disobedient unruly\n\
disobliging uncooperative\n\
disordered unordered\n\
disorderly jumbled\n\
disorganised disorganized\n\
disparateness distinctiveness\n\
dispassion dispassionateness dryness\n\
dispatcher starter\n\
dispel disperse dissipate scatter\n\
dispensability dispensableness\n\
disperse dissipate scatter\n\
disperse dot dust scatter sprinkle\n\
dispersion distribution\n\
dispirited listless\n\
displace move\n\
display presentation\n\
disposed fain inclined prepared\n\
disposition temperament\n\
dispossessed homeless roofless\n\
disproportional disproportionate\n\
disquiet unease uneasiness\n\
disquieted distressed disturbed worried\n\
disregarded forgotten\n\
disreputability disreputableness unrespectability\n\
disruptive riotous troubled tumultuous turbulent\n\
dissembler dissimulator hypocrite phoney phony pretender\n\
disseminator propagator\n\
dissentient dissenting dissident\n\
dissentient recusant\n\
dissentious divisive factious\n\
dissident heretical heterodox\n\
dissimilar unalike\n\
dissimilarity unsimilarity\n\
dissimilitude unlikeness\n\
dissociable separable severable\n\
dissolubility solubleness\n\
dissoluble dissolvable\n\
dissoluteness incontinence\n\
dissolvent dissolver resolvent solvent\n\
dissonant unresolved\n\
distaff female\n\
distance length\n\
distance outdistance outstrip\n\
distant remote\n\
distant remote removed\n\
distasteful unsavory unsavoury\n\
distastefulness odiousness offensiveness\n\
distillate distillation\n\
distinct distinguishable\n\
distinct trenchant\n\
distinctive typical\n\
distinctiveness peculiarity speciality specialness specialty\n\
distinctness otherness separateness\n\
distinguished imposing magisterial\n\
distort twine\n\
distorted misrepresented perverted twisted\n\
distracted distrait\n\
distraught overwrought\n\
distressed dysphoric unhappy\n\
distressed stressed\n\
distressful distressing disturbing perturbing troubling worrisome worrying\n\
distressfulness seriousness\n\
distressingness painfulness\n\
distribute stagger\n\
distributer distributor\n\
district dominion territory\n\
distrust distrustfulness mistrust\n\
disturb touch\n\
disturbed maladjusted\n\
disunite divide\n\
disused obsolete\n\
ditch trench\n\
ditchmoss elodea pondweed\n\
dittany fraxinella\n\
divan diwan\n\
dive honkytonk\n\
dive plunge plunk\n\
diver frogman\n\
diver loon\n\
diver plunger\n\
divergent diverging\n\
divers diverse\n\
diverse various\n\
diverseness diversity multifariousness variety\n\
diversionist saboteur wrecker\n\
divide watershed\n\
divided shared\n\
divider partition\n\
divider splitter\n\
divinatory mantic sibyllic sibylline vatic vatical\n\
divisor factor\n\
dizygotic dizygous\n\
dizzy giddy vertiginous woozy\n\
djakarta jakarta\n\
djinn djinni djinny genie jinnee jinni\n\
dnipropetrovsk yekaterinoslav\n\
dobrich tolbukhin\n\
dobson dobsonfly\n\
dobson hellgrammiate\n\
doc doctor md medico physician\n\
docile teachable\n\
docker dockhand dockworker loader longshoreman lumper stevedore\n\
doctoral doctorial\n\
doctrinaire dogmatist\n\
documental documentary\n\
doddering doddery gaga senile\n\
dodger fox slyboots\n\
dodo fogey fogy fossil\n\
dogged dour persistent pertinacious tenacious unyielding\n\
doggedness perseverance persistence persistency pertinacity tenaciousness tenacity\n\
dogging persisting\n\
doghouse kennel\n\
dogie dogy leppy\n\
dogmatic dogmatical\n\
dogsled mush\n\
doily doyley doyly\n\
dolabrate dolabriform\n\
doleful mournful\n\
dolichocephalic dolichocranial dolichocranic\n\
dolichocephalism dolichocephaly\n\
doll dolly\n\
dollarfish horsefish horsehead moonfish\n\
dolorous dolourous lachrymose tearful weeping\n\
dolphin dolphinfish mahimahi\n\
dolphinfish mahimahi\n\
dolt dullard pillock stupe stupid\n\
domed vaulted\n\
domestic domesticated\n\
domestication tameness\n\
dominance laterality\n\
dominant predominant prevailing prevalent rife\n\
dominated henpecked\n\
domine dominee dominie dominus\n\
domineeringness imperiousness overbearingness\n\
dominick dominique\n\
don preceptor\n\
donbas donbass\n\
done through\n\
donetsk donetske stalino\n\
donjon dungeon keep\n\
donnean donnian\n\
donut doughnut sinker\n\
doob kweek\n\
doodle scrabble scribble\n\
doomed fated\n\
doomed unlucky\n\
door doorway threshold\n\
doorcase doorframe\n\
doorhandle doorknob\n\
doorjamb doorpost\n\
doorkeeper doorman gatekeeper ostiary porter\n\
doorkeeper ostiarius ostiary\n\
doorkeeper usher\n\
doorknocker knocker rapper\n\
doormat weakling wuss\n\
doorsill doorstep threshold\n\
doorstop doorstopper\n\
dopamine dopastat intropin\n\
dope gage grass locoweed sens sess skunk smoke weed\n\
doped drugged narcotised narcotized\n\
doriden glutethimide\n\
dorm dormitory hall\n\
dormant hibernating torpid\n\
dormant inactive\n\
dormant sleeping\n\
dormie dormy\n\
dory walleye\n\
dosage dose\n\
dosemeter dosimeter\n\
dossal dossel\n\
dosshouse flophouse\n\
dostoevski dostoevsky dostoyevsky\n\
dostoevskian dostoyevskian\n\
dotrel dotterel\n\
dotted flecked specked speckled stippled\n\
doubled twofold\n\
doubt doubtfulness dubiousness question\n\
doubter sceptic skeptic\n\
doubtful dubious\n\
doubtful dubious dubitable\n\
doubtful tentative\n\
doubting questioning sceptical skeptical\n\
doughy soggy\n\
doula monitrice\n\
dour forbidding\n\
dour glowering glum moody morose saturnine sullen\n\
doura dourah durra\n\
douse duck\n\
douse dunk plunge souse\n\
dove peacenik\n\
dove squab\n\
dovish pacifist pacifistic\n\
dowdiness drabness homeliness\n\
dowding dowdy\n\
dowdy frumpish frumpy\n\
dowdy pandowdy\n\
dowel joggle\n\
downfall precipitation\n\
downiness featheriness fluffiness\n\
downlike downy flossy fluffy\n\
downrightness straightforwardness\n\
downstair downstairs\n\
downwind lee\n\
downy puberulent pubescent sericeous\n\
dowser rhabdomancer\n\
dowser waterfinder\n\
doxycycline vibramycin\n\
dozy drowsing drowsy\n\
drab dreary\n\
drab sober somber sombre\n\
dracaenaceae dracenaceae\n\
drachm drachma dram\n\
drachm fluidram\n\
draco dragon\n\
draft draught\n\
draft draught potation tipple\n\
draftsman draftsperson draughtsman\n\
draftsman drawer\n\
drafty draughty\n\
drag dredge\n\
drag scuff\n\
drag trail\n\
dragger puller tugger\n\
dragger trawler\n\
dragnet trawl\n\
dragon firedrake\n\
dragon tartar\n\
drain drainpipe\n\
drained knackered\n\
draining exhausting\n\
dramatic spectacular striking\n\
dramatist playwright\n\
dramaturgic dramaturgical\n\
drawknife drawshave\n\
drawstring string\n\
dreadnaught dreadnought\n\
dreamer escapist\n\
dreamer idealist\n\
dreamlike surreal\n\
dreamy lackadaisical languid languorous\n\
dreamy moony woolgathering\n\
dreck schlock shlock\n\
dregs settlings\n\
dressed polished\n\
dresser vanity\n\
dressing stuffing\n\
dressmaker modiste needlewoman seamstress sempstress\n\
drib driblet\n\
dribble drip\n\
dribble drivel drool slobber\n\
dribble filter trickle\n\
dribbler driveller drooler slobberer\n\
drier dryer\n\
drinkable potable\n\
drinker imbiber juicer toper\n\
drippiness mawkishness mushiness sentimentality sloppiness soupiness\n\
drippy drizzly\n\
dripstone hoodmold hoodmould\n\
driveller jabberer\n\
driven goaded\n\
driven impelled\n\
driving impulsive\n\
drizzle mizzle\n\
drizzle moisten\n\
droop flag sag swag\n\
droop sag\n\
drooping droopy sagging\n\
drooping flagging\n\
dropping falling\n\
droppings dung muck\n\
dropsical edematous\n\
droshky drosky\n\
dross impurity\n\
dross scoria slag\n\
drover herder herdsman\n\
drown overwhelm submerge\n\
drowsy oscitant yawning\n\
drudge hack hacker\n\
drudge navvy peon\n\
drudging laboring labouring toiling\n\
drugstore pharmacy\n\
drum drumfish\n\
drum membranophone tympan\n\
drumbeater partisan zealot\n\
drumhead summary\n\
drunk drunkard inebriate rummy sot wino\n\
drunk inebriated intoxicated ripped\n\
drunk intoxicated\n\
druse druze\n\
dry ironic ironical wry\n\
dry juiceless\n\
dry prohibitionist\n\
dry teetotal\n\
dryness sobriety\n\
drywall wallboard\n\
dual duple\n\
dual threefold treble twofold\n\
dualistic manichaean\n\
dubrovnik ragusa\n\
duchy dukedom\n\
duckbill paddlefish\n\
duckbill platypus\n\
ductile malleable\n\
ductile malleable pliable pliant tensile tractile\n\
ductileness ductility\n\
ductule ductulus\n\
dud flop washout\n\
duds threads togs\n\
dueler duelist dueller duellist\n\
duffel duffle\n\
dugout pirogue\n\
dulcet honeyed mellifluous mellisonant\n\
dulcinea ladylove\n\
dulled greyed\n\
dumb mute\n\
dumb speechless\n\
dumbfounded dumbstricken dumbstruck dumfounded flabbergasted stupefied thunderstruck\n\
dump dumpsite wasteyard\n\
dump plunge\n\
dumper tipper\n\
dumpiness squattiness\n\
dumpling dumplings\n\
dumpy podgy pudgy tubby\n\
dumuzi tammuz\n\
dun fawn\n\
dunkard dunker tunker\n\
dunkerque dunkirk\n\
dunnock sparrow\n\
duodecimal twelfth\n\
dupe victim\n\
duplicability reproducibility\n\
duplicable duplicatable\n\
duplicate duplication\n\
duplicate matching twin twinned\n\
durability enduringness lastingness\n\
durable indestructible perdurable undestroyable\n\
durable lasting\n\
durabolin kabolin nandrolone\n\
duramen heartwood\n\
duration length\n\
durazzo durres\n\
durham shorthorn\n\
durian durion\n\
dusanbe dushanbe dyushambe stalinabad\n\
dusky swart swarthy\n\
dusky twilight twilit\n\
dustcloth duster dustrag\n\
duster gabardine gaberdine smock\n\
duster sandstorm sirocco\n\
dustman garbageman\n\
dustpan dustpanful\n\
dutchman hollander netherlander\n\
duteous dutiful\n\
duvet eiderdown\n\
dvd videodisc videodisk\n\
dwarf gnome\n\
dwarf midget nanus\n\
dweeb grind nerd swot wonk\n\
dwindling tapering\n\
dy dysprosium\n\
dye dyestuff\n\
dyeweed greenweed whin woadwaxen woodwaxen\n\
dynamic dynamical\n\
dynamism heartiness vigor vigour\n\
dynamism oomph pizzaz pizzazz zing\n\
dynamiter dynamitist\n\
dynamometer ergometer\n\
dysfunctional nonadaptive\n\
dyslectic dyslexic\n\
eadwig edwy\n\
eagerness forwardness readiness zeal\n\
eardrum myringa tympanum\n\
earflap earlap\n\
earlier earliest\n\
early former other\n\
earmark hallmark stylemark trademark\n\
earnest sincere solemn\n\
earnestness seriousness sincerity\n\
earphone earpiece headphone phone\n\
earreach earshot hearing\n\
earth globe world\n\
earthball puffball\n\
earthbound pedestrian prosaic prosy\n\
earthling earthman tellurian worldling\n\
earthnut goober groundnut peanut\n\
earthnut truffle\n\
earthy vulgar\n\
ease easiness simpleness simplicity\n\
ease informality\n\
east orient\n\
eastbound eastward\n\
easter easterly\n\
easterly eastern\n\
easternmost eastmost\n\
easygoing leisurely\n\
eatage forage grass pasturage pasture\n\
eater feeder\n\
eatery restaurant\n\
eb ebit exabit\n\
eb eib exabyte exbibyte\n\
eb exabyte\n\
ebionite nazarene\n\
ebon ebony\n\
ebonite vulcanite\n\
ebony sable\n\
ebullience enthusiasm exuberance\n\
ebullient exuberant\n\
eccentric flake geek oddball\n\
eccentric nonconcentric\n\
ecclesiastic ecclesiastical\n\
ecdysiast peeler stripper striptease stripteaser\n\
echo replication reverberation\n\
echogram sonogram\n\
echoic echolike\n\
echoic imitative onomatopoeic onomatopoeical onomatopoetic\n\
echoing reechoing\n\
echt genuine\n\
eclat pomp\n\
eclectic eclecticist\n\
ecologic ecological\n\
econometrician econometrist\n\
economic economical\n\
economical frugal scotch sparing stinting\n\
economiser economizer\n\
economy thriftiness\n\
ecstatic enraptured rapt rapturous rhapsodic\n\
ectoblast ectoderm exoderm\n\
ectodermal ectodermic\n\
ectoparasite ectozoan ectozoon epizoan epizoon\n\
ectotherm poikilotherm\n\
ectothermic heterothermic poikilothermic poikilothermous\n\
ectozoan epizoan\n\
ecuadoran ecuadorian\n\
ecumenic ecumenical oecumenic oecumenical\n\
edacious esurient rapacious ravening ravenous voracious wolfish\n\
edacity esurience rapaciousness rapacity voraciousness voracity\n\
eddy purl swirl whirlpool\n\
eden heaven nirvana paradise\n\
edental edentate edentulate\n\
edgy jittery jumpy nervy overstrung restive uptight\n\
edibility edibleness\n\
edifying enlightening\n\
edited emended\n\
edo tokio tokyo yeddo yedo\n\
edronax reboxetine\n\
educated enlightened\n\
educatee pupil student\n\
educationalist educationist\n\
educator pedagog pedagogue\n\
eelpout pout\n\
eerie eery\n\
eeriness ghostliness\n\
efface erase\n\
effaceable erasable\n\
effect impression\n\
effecter effector\n\
effective effectual efficacious\n\
effective efficient\n\
effectiveness effectivity effectuality effectualness\n\
effectiveness potency\n\
effectual legal\n\
effeminacy effeminateness sissiness softness unmanliness womanishness\n\
efferent motorial\n\
effervescent sparkling\n\
efficaciousness efficacy\n\
effigy image simulacrum\n\
effluent outflowing\n\
effluent wastewater\n\
effulgence radiance radiancy refulgence refulgency shine\n\
effusive gushing gushy\n\
effusiveness expansiveness expansivity\n\
egalitarian equalitarian\n\
egg eggs\n\
eggar egger\n\
eggbeater eggwhisk\n\
eggshell shell\n\
egocentric egoist\n\
egocentric egoistic egoistical\n\
egocentrism egoism\n\
egoist egotist swellhead\n\
egotistic egotistical narcissistic\n\
eibit exbibit\n\
eight eighter octad octet octonary ogdoad viii\n\
eight viii\n\
eightfold octuple\n\
einsteinium es\n\
eire ireland\n\
eisenhower ike\n\
eject exclude\n\
eject squirt\n\
ejector ouster\n\
el elevated\n\
elaborate luxuriant\n\
elaborateness elaboration intricacy involution\n\
elaborateness ornateness\n\
elam susiana\n\
elapse lapse\n\
elasmobranch selachian\n\
elasmobranchii selachii\n\
elastic flexible pliable pliant\n\
elasticised elasticized\n\
elated gleeful joyful jubilant\n\
elater elaterid\n\
elating exhilarating\n\
elder older\n\
elder senior\n\
eldest firstborn\n\
eldritch uncanny unearthly weird\n\
elect elite\n\
elected elective\n\
elective facultative\n\
elector voter\n\
electric electrical\n\
electric galvanic galvanising galvanizing\n\
electrician lineman linesman\n\
electrifying thrilling\n\
electron negatron\n\
electronegative negative\n\
electronegativity negativity\n\
electroneutral neutral\n\
electrostatic static\n\
elegant graceful refined\n\
elementary uncomplicated unproblematic\n\
elephantine gargantuan giant jumbo\n\
elevated exalted idealistic lofty rarefied rarified sublime\n\
elevated raised\n\
elfin elfish elvish\n\
elfin elflike\n\
elfin fey\n\
elia lamb\n\
elicited evoked\n\
elisabethville lubumbashi\n\
elixophyllin theobid theophylline\n\
elk moose\n\
elk wapiti\n\
ellas greece\n\
ellipse oval\n\
ellipsoid ellipsoidal spheroidal\n\
elliptic elliptical\n\
elliptic elliptical oval ovate oviform ovoid prolate\n\
ellipticity oblateness\n\
elm elmwood\n\
elongate elongated\n\
elongate linear\n\
elongated extended lengthened prolonged\n\
elongation extension\n\
eloquent facile fluent\n\
elusive subtle\n\
elysian inspired\n\
em pica\n\
emancipated liberated\n\
emancipator manumitter\n\
emasculated gelded\n\
embark ship\n\
embarrassed humiliated mortified\n\
embarrassing mortifying\n\
embarrassment overplus plethora superfluity\n\
embed engraft imbed implant plant\n\
embezzled misappropriated\n\
embiodea embioptera\n\
emblematic emblematical symbolic symbolical\n\
emblematic exemplary typic\n\
embonpoint plumpness roundness\n\
embossment relief relievo rilievo\n\
embouchure mouthpiece\n\
embrasure port porthole\n\
embrocation liniment\n\
embroidery fancywork\n\
embroiled entangled\n\
embryologic embryonal embryonic\n\
embryonic embryotic\n\
emcee host\n\
emergent emerging\n\
emerging rising\n\
emeside ethosuximide zarontin\n\
emetic nauseant vomit vomitive\n\
emf voltage\n\
emigrant emigre emigree outgoer\n\
eminence tubercle tuberosity\n\
eminent high\n\
eminent lofty soaring towering\n\
emissary envoy\n\
emmental emmentaler emmenthal emmenthaler\n\
emotionalism emotionality\n\
emotionless passionless\n\
emotionlessness unemotionality\n\
empale impale spike transfix\n\
empathetic empathic\n\
emphasis vehemence\n\
emphasised emphasized emphatic\n\
emphatic exclamatory\n\
emphatic forceful\n\
empire imperium\n\
empiric empirical\n\
emplane enplane\n\
employed utilised utilized\n\
empowered sceptered sceptred\n\
emptiness vacancy vacuum void\n\
emptiness vanity\n\
empty hollow vacuous\n\
empurpled purple\n\
empyreal empyrean\n\
empyreal empyrean sublime\n\
empyrean firmament heavens sphere welkin\n\
emulous rivalrous\n\
en nut\n\
enalapril vasotec\n\
enamored infatuated potty smitten\n\
enantiomer enantiomorph\n\
enate enatic maternal\n\
enate matrikin matrisib\n\
enbrel etanercept\n\
encainide enkaid\n\
encase incase\n\
enceinte expectant gravid great\n\
encephalogram pneumoencephalogram\n\
enchantress witch\n\
encircle gird\n\
encircled surrounded\n\
encircling skirting\n\
enclose enfold envelop enwrap wrap\n\
enclose inclose\n\
encomiastic eulogistic panegyric panegyrical\n\
encounter meet see\n\
encouraging supporting\n\
encroach impinge infringe\n\
encroacher invader\n\
encroaching invasive trespassing\n\
encrust incrust\n\
encrustation incrustation\n\
encumbrance hinderance hindrance incumbrance interference preventative preventive\n\
encyclopaedic encyclopedic\n\
encyclopaedist encyclopedist\n\
end oddment remainder remnant\n\
end terminal\n\
endecott endicott\n\
endemic endemical\n\
endermatic endermic\n\
endive escarole\n\
endive witloof\n\
endless eternal interminable\n\
endoblast endoderm entoblast entoderm hypoblast\n\
endocarp stone\n\
endocrinal endocrine\n\
endocrine hormone\n\
endogamic endogamous\n\
endogen liliopsid monocot monocotyledon\n\
endogenetic endogenic\n\
endogenic endogenous\n\
endomorphic pyknic\n\
endoparasite endozoan entoparasite entozoan entozoon\n\
endoprocta entoprocta\n\
endorser indorser\n\
endorser indorser ratifier subscriber\n\
endothermal endothermic\n\
endovenous intravenous\n\
endozoan entozoan\n\
endozoic entozoan entozoic\n\
endpoint termination terminus\n\
enemy foe\n\
enemy foe foeman opposition\n\
energetic gumptious industrious\n\
energid protoplast\n\
energiser energizer\n\
energising energizing kinetic\n\
energy muscularity vigor vigour vim\n\
energy vigor vigour zip\n\
enflurane ethrane\n\
enforced implemented\n\
enfranchisement franchise\n\
engage lock mesh operate\n\
engaged intermeshed meshed\n\
engaged occupied\n\
engaging piquant\n\
engine locomotive\n\
engineer technologist\n\
engraft graft ingraft\n\
engrave etch\n\
engrave inscribe\n\
engraved etched graven incised inscribed\n\
engrossment intentness\n\
enigmatic enigmatical puzzling\n\
enigmatic oracular\n\
enjoyable gratifying pleasurable\n\
enjoyment use\n\
enkindled ignited kindled\n\
enlace entwine interlace intertwine lace twine\n\
enlarged exaggerated magnified\n\
enlarged hypertrophied\n\
enlightening illuminating informative\n\
enlistee recruit\n\
enlivened spirited\n\
enlivener invigorator quickener\n\
enmesh ensnarl mesh\n\
enmeshed intermeshed\n\
ennead ix nine niner\n\
ennobling exalting\n\
enologist fermentologist oenologist\n\
enophile oenophile\n\
enormity outrageousness\n\
enormous tremendous\n\
enormousness grandness greatness immenseness immensity sizeableness vastness wideness\n\
enough sufficiency\n\
ensconce settle\n\
enshrine shrine\n\
enshroud hide shroud\n\
ensilage silage\n\
ensnare entrap snare trammel trap\n\
entangle snarl tangle\n\
entanglement web\n\
entellus hanuman\n\
enter infix insert introduce\n\
enteral enteric\n\
enteral enteric intestinal\n\
enterics enterobacteria entric\n\
enteroceptor interoceptor\n\
enterprise enterprisingness initiative\n\
enterpriser entrepreneur\n\
enthusiast fancier\n\
enthusiast partisan partizan\n\
enticement lure\n\
entire intact\n\
entire intact integral\n\
entire stallion\n\
entire total\n\
entomologic entomological\n\
entrails innards viscera\n\
entrance entranceway entree entry entryway\n\
entrant fledgeling fledgling freshman neophyte newbie newcomer starter\n\
entrench intrench\n\
entrenchment intrenchment\n\
entresol mezzanine\n\
entropy information\n\
entropy randomness\n\
enured hardened inured\n\
envelope gasbag\n\
environ ring skirt surround\n\
environment environs surround surroundings\n\
environs purlieu\n\
envisioned pictured visualised visualized\n\
eosinophil eosinophile\n\
eparchy exarchate\n\
epaulet epaulette\n\
epenthetic parasitic\n\
epha ephah\n\
ephemeral ephemeron\n\
ephemeral fugacious passing transient transitory\n\
ephemerality ephemeralness fleetingness\n\
ephemerid ephemeropteran\n\
ephemerida ephemeroptera\n\
epic epical\n\
epic heroic\n\
epicarp exocarp\n\
epicenter epicentre\n\
epicure epicurean foodie gastronome gourmet\n\
epicurean hedonic hedonistic\n\
epicurean luxuriant luxurious sybaritic voluptuary voluptuous\n\
epicyclic epicyclical\n\
epideictic epideictical\n\
epidemiologic epidemiological\n\
epidural extradural\n\
epigon epigone\n\
epiphyseal epiphysial\n\
episcopal episcopalian\n\
episcopal pontifical\n\
episode sequence\n\
episodic occasional\n\
episperm testa\n\
epistemic epistemological\n\
epistolary epistolatory\n\
eponymic eponymous\n\
eq equivalent\n\
equable placid\n\
equaliser equalizer\n\
equanil meprin meprobamate miltown\n\
equestrian horseman\n\
equid equine\n\
equipage materiel\n\
equipped equipt\n\
equipped furnished\n\
equipped weaponed\n\
equisetatae sphenopsida\n\
equitable just\n\
equity fairness\n\
equivalent tantamount\n\
equivocation evasiveness prevarication\n\
equivocator hedger tergiversator\n\
er erbium\n\
eradicator exterminator terminator\n\
erect rear\n\
erect tumid\n\
erect upright vertical\n\
erectness uprightness\n\
erectness uprightness verticality verticalness\n\
eremitic eremitical\n\
ereshkigal ereshkigel\n\
eringo eryngo\n\
erinyes eumenides fury\n\
eristic eristical\n\
erivan jerevan yerevan\n\
ern erne\n\
eroded scoured\n\
erose jagged jaggy notched toothed\n\
erotic titillating\n\
err stray\n\
erratic fickle mercurial quicksilver\n\
erratic planetary wandering\n\
erratic temperamental\n\
erroneousness error\n\
error wrongdoing\n\
erstwhile former old onetime quondam sometime\n\
erudite learned\n\
eruptive igneous\n\
erythrocin erythromycin ethril ilosone pediamycin\n\
erythrocyte rbc\n\
erythrocytolysin erythrolysin haemolysin hemolysin\n\
erythroxylon erythroxylum\n\
escallop scallop scollop\n\
escargot snail\n\
escarp escarpment scarp\n\
escarpment scarp\n\
eschalot shallot\n\
escort see\n\
escritoire secretaire secretary\n\
escutcheon scutcheon\n\
esidrix hydrochlorothiazide hydrodiuril microzide\n\
eskalith lithane lithonate\n\
eskimo esquimau inuit\n\
esophagoscope oesophagoscope\n\
esophagus gorge gullet oesophagus\n\
espana spain\n\
esparcet sainfoin sanfoin\n\
especial exceptional particular\n\
esq esquire\n\
essayist litterateur\n\
essence perfume\n\
essential indispensable\n\
essential necessary necessity requirement requisite\n\
essential substantive\n\
essentiality essentialness\n\
essonite hessonite\n\
establish instal install\n\
established naturalized\n\
estazolam prosom\n\
esteemed honored prestigious\n\
esthonia estonia\n\
estimable honorable respectable\n\
estradiol oestradiol\n\
estragon tarragon\n\
estriol oestriol\n\
estrogen oestrogen\n\
estrone estronol oestrone theelin\n\
estuarial estuarine\n\
esurient famished ravenous starved\n\
etamin etamine\n\
ethanediol glycol\n\
ethchlorvynol placidyl\n\
ethene ethylene\n\
ether ethoxyethane\n\
ether quintessence\n\
ethereal gossamer\n\
ethical honorable honourable\n\
ethician ethicist\n\
ethnic heathen heathenish pagan\n\
ethnographic ethnographical\n\
ethnologic ethnological\n\
ethocaine procaine\n\
etodolac lodine\n\
eu europium\n\
eubacteria eubacterium\n\
eucalypt eucalyptus\n\
eucarya fusanus\n\
eucaryote eukaryote\n\
eucaryotic eukaryotic\n\
euclidean euclidian\n\
eudaemon eudemon\n\
eudaemonic eudemonic\n\
euglenid euglenoid euglenophyte\n\
eulogist panegyrist\n\
euphonic euphonical\n\
euphonious euphonous\n\
eurasian eurasiatic\n\
eurocentric europocentric\n\
euronithopod euronithopoda ornithopoda\n\
eutherian placental\n\
evaluator judge\n\
evangelical evangelistic\n\
evangelist gospeler gospeller revivalist\n\
evaporable vaporific vaporizable vapourific vapourisable volatilisable volatilizable\n\
even flush\n\
even tied\n\
eveningwear formalwear\n\
evenk tungus\n\
evenki ewenki\n\
evenness invariability\n\
everyday mundane quotidian routine unremarkable workaday\n\
evidential evidentiary\n\
evil evilness\n\
evil malefic malevolent malign\n\
evil vicious\n\
evildoer sinner\n\
eviscerate resect\n\
evocative redolent remindful reminiscent resonant\n\
ewer pitcher\n\
exacting exigent\n\
exacting fastidious\n\
exacting strict\n\
exactitude exactness\n\
exaggerated overdone overstated\n\
examinee testee\n\
examiner inspector\n\
examiner quizzer tester\n\
exanimate lifeless\n\
exasperating infuriating maddening vexing\n\
exceeding exceptional olympian prodigious surpassing\n\
excellent fantabulous ripping splendid\n\
exceptionable objectionable\n\
excess excessiveness inordinateness\n\
excess nimiety surplus surplusage\n\
excess redundant supererogatory superfluous supernumerary surplus\n\
excessive extravagant exuberant overweening\n\
excessive inordinate undue unreasonable\n\
exchangeability fungibility interchangeability interchangeableness\n\
exchangeable interchangeable similar standardised standardized\n\
excise expunge strike\n\
exciseman taxman\n\
excitability excitableness volatility\n\
excitable irritable\n\
excitant excitative excitatory\n\
excitant stimulant\n\
exclusive single undivided\n\
exclusive sole\n\
excrement excreta excretion\n\
excursionist rubberneck sightseer tripper\n\
excusable forgivable venial\n\
excuser forgiver pardoner\n\
executable feasible practicable viable workable\n\
exegetic exegetical\n\
exemplary model\n\
exemplifying illustrative\n\
exempt nontaxable\n\
exfoliation scale scurf\n\
exhalation halitus\n\
exhaust fumes\n\
exhausted fagged fatigued spent\n\
exhausted spent\n\
exhausting tiring wearing wearying\n\
exhaustive thorough thoroughgoing\n\
exhibit march parade\n\
exhibitioner exhibitor shower\n\
exhibitionist flasher\n\
exhilarated gladdened\n\
exhilarating stimulating\n\
exhortative exhortatory hortative hortatory\n\
exiguity leanness meagerness meagreness poorness scantiness scantness\n\
exile expat expatriate\n\
existent existing\n\
existential experiential\n\
exit issue outlet\n\
exit leave\n\
exogamic exogamous\n\
exogenic exogenous\n\
exopterygota hemimetabola\n\
exorbitance outrageousness\n\
exorbitant extortionate outrageous steep unconscionable usurious\n\
exorcise exorcize\n\
exorciser exorcist\n\
exothermal exothermic\n\
exoticism exoticness exotism\n\
expandable expandible expansible\n\
expandable expandible expansible expansile\n\
expansive heroic\n\
expansive talkative\n\
expansiveness expansivity\n\
expectorant expectorator\n\
expectorator spitter\n\
expedience expediency\n\
expedience opportunism\n\
expendable spendable\n\
experienced experient\n\
experimental observational\n\
expert technical\n\
expiative expiatory propitiatory\n\
explainable interpretable\n\
explicit expressed\n\
exploitative exploitatory exploitive\n\
exploited used victimised victimized\n\
exploiter user\n\
explorative exploratory\n\
explosive volatile\n\
export exportation\n\
expose uncover\n\
exposed uncovered\n\
expositive expository\n\
expositor expounder\n\
exposure photo photograph pic picture\n\
express extract\n\
express limited\n\
expressed uttered verbalised verbalized\n\
expressionist expressionistic\n\
expressway freeway motorway pike superhighway throughway thruway\n\
exquisite keen\n\
exquisite recherche\n\
extemporaneous extemporary extempore impromptu offhand offhanded unrehearsed\n\
extend gallop\n\
extend stretch unfold\n\
extendable extendible\n\
extended extensive\n\
extended lengthy prolonged protracted\n\
extensible extensile\n\
extension lengthiness prolongation\n\
extensiveness largeness\n\
exterminable extirpable\n\
external extraneous\n\
external international\n\
externality outwardness\n\
exterritorial extraterritorial\n\
extinct nonextant\n\
extinct out\n\
extoller laudator lauder\n\
extract infusion\n\
extractable extractible\n\
extralegal nonlegal\n\
extraneous foreign\n\
extraneous immaterial impertinent orthogonal\n\
extraordinary sinful\n\
extrasensory paranormal\n\
extravagance extravagancy\n\
extravagance prodigality profligacy\n\
extravagant prodigal profligate spendthrift\n\
extraversion extroversion\n\
extraversive extroversive\n\
extravert extraverted extravertive extrovert extroverted extrovertive\n\
extravert extrovert\n\
extreme extremum\n\
extreme utmost uttermost\n\
extremist radical ultra\n\
extrospective extroverted\n\
extroverted forthcoming outgoing\n\
exuberant luxuriant profuse riotous\n\
exudate exudation\n\
exultant exulting jubilant prideful rejoicing triumphal triumphant\n\
eye oculus optic\n\
eyeball orb\n\
eyebath eyecup\n\
eyeglass monocle\n\
eyeglasses glasses specs spectacles\n\
eyehole eyelet\n\
eyehole peephole spyhole\n\
eyeless sightless unseeing\n\
eyelid lid palpebra\n\
eyepiece ocular\n\
eyeshot view\n\
eyespot ocellus\n\
eyra jaguarondi jaguarundi\n\
ezechiel ezekiel\n\
ezekias hezekiah\n\
ezo hokkaido yezo\n\
fab fabulous\n\
fabaceae leguminosae\n\
fabled legendary\n\
fabric framework\n\
fabricated fancied fictional fictitious\n\
fabricator fibber storyteller\n\
fabulous mythic mythical mythologic mythological\n\
facade frontage frontal\n\
face side\n\
facia fascia\n\
facile neat\n\
facility installation\n\
facility readiness\n\
facing veneer\n\
facsimile fax\n\
factory manufactory mill\n\
factuality factualness\n\
faddish faddy\n\
faecal fecal\n\
faerie faery fairy fay sprite\n\
faeroes faroes\n\
fag faggot fagot fairy nance pansy poof poove pouf queen queer\n\
faggot fagot\n\
faggoting fagoting\n\
failure loser nonstarter\n\
faineance idleness\n\
faineant indolent lazy otiose slothful\n\
fainthearted timid\n\
faintheartedness faintness\n\
fairish reasonable\n\
faisalabad lyallpur\n\
faithfulness fidelity\n\
faithless traitorous treasonable treasonous unfaithful\n\
faithlessness falseness fickleness inconstancy\n\
fake faker fraud imposter impostor pretender pseud pseudo sham shammer\n\
fake faux imitation simulated\n\
fake postiche sham\n\
fakeer fakir faqir faquir\n\
falafel felafel\n\
falangist phalangist\n\
falcate falciform\n\
falconer hawker\n\
falderol folderal frill gimcrack gimcrackery nonsense trumpery\n\
falkner faulkner\n\
fall flow hang\n\
fallacious unsound\n\
faller feller logger lumberjack lumberman\n\
fallible frail imperfect\n\
fallopio fallopius\n\
falls waterfall\n\
falseness hollowness insincerity\n\
falter waver\n\
familial genetic hereditary inherited transmissible transmitted\n\
familiarising familiarizing\n\
familiarity intimacy\n\
family kin kinsperson\n\
famotidine pepcid\n\
fan rooter\n\
fanatic fanatical overzealous rabid\n\
fanatic fiend\n\
fanciful imaginary notional\n\
fanciful notional\n\
fanjet turbofan turbojet\n\
fanlight skylight\n\
fanlight transom\n\
fantast futurist\n\
fantastic fantastical\n\
fantastic howling marvellous marvelous rattling terrific tremendous wonderful wondrous\n\
fanweed stinkweed\n\
farawayness farness remoteness\n\
farce forcemeat\n\
farcical ludicrous ridiculous\n\
farinaceous grainy granular granulose gritty mealy\n\
farkleberry sparkleberry\n\
farmer granger husbandman sodbuster\n\
farmhand fieldhand\n\
farmland ploughland plowland tillage tilth\n\
farmplace farmstead\n\
farrier horseshoer\n\
farseeing farsighted foresighted foresightful long longsighted prospicient\n\
farseeing longsighted\n\
farsighted presbyopic\n\
farther further\n\
farthermost farthest furthermost furthest utmost uttermost\n\
fascicle fasciculus\n\
fascinated hypnotised hypnotized mesmerised mesmerized spellbound transfixed\n\
fascist fascistic\n\
fashion manner mode style way\n\
fashionable stylish\n\
fast flying\n\
fast immobile\n\
fast loyal truehearted\n\
fasten fix\n\
fastened tied\n\
fastener fastening fixing holdfast\n\
fastness fixedness fixity fixture secureness\n\
fastness speed swiftness\n\
fastness stronghold\n\
fat fatty\n\
fat fertile productive rich\n\
fat juicy\n\
fatal fateful\n\
fatalist fatalistic\n\
fateful foreboding portentous\n\
fatherland homeland motherland\n\
fatherlike fatherly\n\
fathom fthm\n\
fathomable plumbable soundable\n\
fatima fatimah\n\
fatless nonfat\n\
faucet spigot\n\
faultless immaculate impeccable incorrupted\n\
faultlessness impeccability\n\
faulty incorrect\n\
faust faustus\n\
fauve fauvist\n\
favor favour\n\
favorable favourable\n\
favorable favourable lucky prosperous\n\
favored favorite favourite pet preferent preferred\n\
favorite favourite\n\
fdr roosevelt\n\
fe iron\n\
fearful frightful\n\
fearful timorous trepid\n\
fearless unafraid\n\
feasibility feasibleness\n\
feather plumage plume\n\
feathered feathery plumy\n\
featherless unfeathered\n\
featherlike feathery\n\
feature lineament\n\
febrile feverish\n\
feckless inept\n\
fecund fertile prolific\n\
fecundity fruitfulness\n\
fed federal\n\
federal union\n\
federate federated\n\
federita feterita\n\
fedora homburg stetson trilby\n\
feeble lame\n\
feeble nerveless\n\
feebleness tenuity\n\
feed provender\n\
feedbag nosebag\n\
feel finger\n\
feel palpate\n\
feist fice\n\
feisty huffy touchy\n\
feisty plucky spunky\n\
feldene piroxicam\n\
feldspar felspar\n\
felicitous happy\n\
felicitousness felicity\n\
felid feline\n\
fell hide\n\
fell vanish\n\
fellata fula fulah fulani fulbe\n\
felloe felly\n\
femaleness feminineness\n\
feminine womanly\n\
femininity muliebrity\n\
feminist libber\n\
femoris femur thighbone\n\
femtometer femtometre fermi\n\
fen fenland marsh marshland\n\
fence fencing\n\
fencer swordsman\n\
fencesitter independent mugwump\n\
fender wing\n\
fenestella lunette\n\
fengtien moukden mukden shenyang\n\
fennel finocchio\n\
fenoprofen nalfon\n\
fentanyl sublimaze\n\
feral ferine\n\
fermium fm\n\
ferned ferny\n\
fernlike ferny\n\
ferocious fierce furious\n\
ferocity fierceness furiousness fury vehemence violence wildness\n\
ferric ferrous\n\
ferry ferryboat\n\
fertile prolific\n\
fertiliser fertilizer\n\
fertility prolificacy rankness richness\n\
fervent fervid\n\
fes fez\n\
fess fesse\n\
festal festive merry\n\
festering ichor purulence pus sanies suppuration\n\
festoon festoonery\n\
fetal foetal\n\
fetching taking winning\n\
fetich fetish hoodoo juju voodoo\n\
fetid foetid funky noisome smelly stinking\n\
fetidness foulness malodorousness rankness stinkiness\n\
fetoprotein foetoprotein\n\
fetoscope foetoscope\n\
fetter hobble\n\
fetter shackle\n\
fettered shackled\n\
fettuccine fettuccini\n\
fetus foetus\n\
feudal feudalistic\n\
feudatory liege liegeman vassal\n\
feverish feverous\n\
feverish hectic\n\
fey touched\n\
fez tarboosh\n\
fiber fibre\n\
fiber roughage\n\
fiberboard fibreboard\n\
fiberglass fibreglass\n\
fiberoptic fibreoptic\n\
fibril filament strand\n\
fibrinolysin plasmin\n\
fibrosity fibrousness\n\
fibrous hempen\n\
fibrous sinewy stringy unchewable\n\
fickle volatile\n\
fictile moldable plastic\n\
fictile pliable\n\
fiddle monkey tamper\n\
fiddle violin\n\
fiddler tinkerer\n\
fiddler twiddler\n\
fiddler violinist\n\
fiddling footling lilliputian niggling petty picayune piddling piffling trivial\n\
fiducial fiduciary\n\
field theater theatre\n\
fielder fieldsman\n\
fieldfare snowbird\n\
fierce tearing vehement violent\n\
fiery flaming\n\
fiery igneous\n\
figural figurative\n\
figurative nonliteral\n\
figurehead front strawman\n\
figurine statuette\n\
filagree filigree fillagree\n\
filament filum\n\
filamentlike filamentous filiform threadlike thready\n\
filet fillet\n\
filibuster filibusterer\n\
filicales polypodiales\n\
filicinae filicopsida\n\
filipino philippine\n\
fill filling\n\
fille girl miss missy\n\
filler makeweight\n\
fillet lemniscus\n\
fillet stopping\n\
fillet taenia tenia\n\
filling weft woof\n\
filter filtrate strain\n\
filter percolate permeate\n\
filthy lousy\n\
filthy nasty\n\
fin flipper\n\
fin louver louvre\n\
fin tailfin\n\
finable fineable\n\
finagler wangler\n\
final net\n\
financial fiscal\n\
financier moneyman\n\
finder viewfinder\n\
fineness powderiness\n\
fineness thinness\n\
finespun hairsplitting\n\
finger thumb\n\
fingerflower fingerroot\n\
fingermark fingerprint\n\
finical finicky fussy particular picky\n\
finished ruined\n\
finland suomi\n\
fiord fjord\n\
fireball powerhouse\n\
firebird hangbird\n\
firebird redbird\n\
firebomb incendiary\n\
firebrand inciter instigant instigator provoker\n\
firebreak fireguard\n\
firefighter fireman\n\
firelock flintlock\n\
fireman reliever\n\
fireman stoker\n\
firenze florence\n\
fireplace hearth\n\
fireroom stokehold stokehole\n\
fireside hearth\n\
firethorn pyracanth pyracantha\n\
fireweed wickup\n\
firework pyrotechnic\n\
firmness resoluteness resolution resolve\n\
firmness soundness\n\
firmness steadiness\n\
fish pisces\n\
fisher fisherman\n\
fisher pekan\n\
fishery piscary\n\
fishgig fizgig gig lance spear\n\
fishmonger fishwife\n\
fishy funny shady suspect suspicious\n\
fissile fissionable\n\
fistful handful\n\
fistula sinus\n\
fistular fistulate fistulous\n\
fit primed\n\
fitch foulmart foumart polecat\n\
fitful interrupted\n\
fitful spasmodic\n\
fitfulness jerkiness\n\
fitness fittingness\n\
fitter healthier\n\
fitting meet\n\
fivefold quintuple\n\
fix posit situate\n\
fixed frozen\n\
fixed rigid\n\
fixedness unalterability\n\
fixer mender repairer\n\
fixer methadon methadone\n\
fixings ingredient\n\
fixings trimmings\n\
fixity immutability immutableness\n\
fixture habitue\n\
fizzing fizzy\n\
fl florida\n\
flabbiness flaccidity limpness\n\
flabby flaccid\n\
flack flak\n\
flag flagstone\n\
flag iris\n\
flagellata mastigophora\n\
flagellate flagellated whiplike\n\
flagellate mastigophoran mastigophore\n\
flagellate scourge\n\
flagellum scourge\n\
flageolet haricot\n\
flagitious heinous\n\
flagpole flagstaff\n\
flagyl metronidazole\n\
flail lam thresh\n\
flail thresh\n\
flair flare\n\
flake peel\n\
flake snowflake\n\
flakey flaky\n\
flamboyance floridity floridness showiness\n\
flamboyant showy splashy\n\
flameflower kniphofia tritoma\n\
flammability inflammability\n\
flammable inflammable\n\
flange rim\n\
flannel gabardine tweed\n\
flannel washcloth washrag\n\
flap flaps\n\
flap undulate wave\n\
flash flashbulb flashgun photoflash\n\
flashboard flashboarding\n\
flashlight torch\n\
flashpoint hotspot\n\
flashy gaudy jazzy showy sporty\n\
flask flaskful\n\
flatbed flatcar\n\
flatbottom flatbottomed\n\
flatfoot patrolman\n\
flatfoot splayfoot\n\
flatness languor lethargy phlegm sluggishness\n\
flatness lusterlessness lustrelessness matt matte\n\
flatness planeness\n\
flattened planate\n\
flatworm platyhelminth\n\
flautist flutist\n\
flavorer flavoring flavourer flavouring seasoner seasoning\n\
flavorful flavorous flavorsome flavourful flavourous flavoursome sapid saporous\n\
flavorlessness flavourlessness savorlessness savourlessness tastelessness\n\
flavorsomeness flavoursomeness savoriness\n\
flawless unflawed\n\
flaxedil gallamine\n\
flaxen sandy\n\
flaxseed linseed\n\
fleabane horseweed\n\
fleawort psyllium\n\
flecainide tambocor\n\
fledged mature\n\
fledged vaned\n\
fledgeless unfledged unvaned\n\
fledgeling fledgling\n\
fleece shear\n\
fleece sheepskin\n\
fleece wool\n\
fleeceable gullible\n\
fleer fugitive runaway\n\
fleet swift\n\
fleeting fugitive momentaneous momentary\n\
flesh pulp\n\
fleshy overweight\n\
fleshy sarcoid\n\
flexibility flexibleness\n\
flexibility tractability tractableness\n\
flexible flexile\n\
flexible whippy\n\
flick riffle\n\
flicker flitter flutter quiver waver\n\
flier flyer\n\
flight trajectory\n\
flighty flyaway scatterbrained\n\
flighty nervous skittish spooky\n\
flimsiness shoddiness\n\
flimsy fragile slight tenuous thin\n\
flimsy insubstantial\n\
flimsy onionskin\n\
flimsy unconvincing\n\
flindosa flindosy\n\
flint flinty granitic obdurate stony\n\
float swim\n\
floatation flotation\n\
floc floccule\n\
flocculent woolly wooly\n\
flog lash lather slash strap trounce welt whip\n\
flogger scourger\n\
flood floodlight photoflood\n\
floodgate penstock sluicegate\n\
floodlighted floodlit\n\
floor flooring\n\
floor storey story\n\
floorwalker shopwalker\n\
floozie floozy hooker hustler slattern streetwalker\n\
flora plant\n\
floral flowered\n\
floret floweret\n\
florid rubicund ruddy sanguine\n\
florin guilder gulden\n\
flotsam jetsam\n\
flounce frill furbelow\n\
flounder stagger\n\
flouter jeerer mocker scoffer\n\
flow flux\n\
flowerless nonflowering\n\
flowery ornate\n\
fluctuate vacillate waver\n\
fluctuation wavering\n\
flue fluke\n\
fluegelhorn flugelhorn\n\
fluidity fluidness\n\
fluidity fluidness liquidity liquidness runniness\n\
fluke trematode\n\
flume gulch\n\
flump plank plonk plop plump plunk\n\
flunitrazepan rohypnol\n\
flunkey flunky lackey\n\
flunkey flunky stooge\n\
fluor fluorite fluorspar\n\
fluorescein fluoresceine resorcinolphthalein\n\
fluoroform trifluoromethane\n\
fluoroscope roentgenoscope\n\
fluoxetine prozac sarafem\n\
flushed rosy\n\
fluster perturbation\n\
flustered perturbed rattled\n\
flute fluting\n\
flutter palpitate\n\
fluvastatin lescol\n\
flyblown maggoty\n\
flyblown sordid squalid\n\
flyover overpass\n\
flyswat flyswatter swatter\n\
foam froth\n\
foaming foamy frothing\n\
focused focussed\n\
foehn fohn\n\
fogged foggy\n\
fogyish mossy stodgy\n\
foible idiosyncrasy mannerism\n\
foil hydrofoil\n\
foil transparency\n\
folacin folate\n\
fold plica\n\
fold sheepcote sheepfold\n\
foldable foldaway folding\n\
foliaceous foliaged foliose\n\
foliaceous foliate foliated\n\
foliage foliation\n\
foliage leaf leafage\n\
foliate foliated\n\
follow pursue\n\
following next\n\
following undermentioned\n\
folly foolishness unwiseness\n\
fomite vehicle\n\
fomor fomorian\n\
fond partial\n\
fondler petter\n\
fondu fondue\n\
fontanel fontanelle\n\
food nutrient\n\
foodstuff grocery\n\
fool jester\n\
fool muggins sap saphead tomfool\n\
foolhardiness rashness recklessness\n\
foolhardy heady rash reckless\n\
foolproof unfailing\n\
foot foundation fundament groundwork substructure understructure\n\
foot ft\n\
foot hoof\n\
foot pes\n\
footbridge overcrossing\n\
footer pedestrian walker\n\
footgear footwear\n\
foothold footing\n\
footlocker locker\n\
footpad padder\n\
footpath pathway\n\
footrest footstool ottoman tuffet\n\
footslog plod slog trudge\n\
footslogger infantryman marcher\n\
footstall pedestal plinth\n\
footstep pace step stride\n\
footsure surefooted\n\
foram foraminifer\n\
foramen hiatus\n\
forbear forebear\n\
forbearance longanimity patience\n\
forbearing longanimous\n\
forbidden out prohibited proscribed taboo tabu verboten\n\
forceless unforceful\n\
forcible physical\n\
forebrain prosencephalon\n\
forecaster predictor prognosticator soothsayer\n\
forefather sire\n\
forefinger index\n\
forehand forehanded\n\
foreign strange\n\
foreigner noncitizen outlander\n\
foreigner outsider\n\
forelady forewoman\n\
foreland headland promontory\n\
forelock foretop\n\
foremost frontmost\n\
foreordained predestinate predestined\n\
forepart front\n\
forerunner precursor\n\
foresight foresightedness foresightfulness\n\
foreskin prepuce\n\
forest timber timberland woodland\n\
forge smithy\n\
forge spirt spurt\n\
forgetful mindless unmindful\n\
forgetful oblivious\n\
forgetful unretentive\n\
forgivingness kindness\n\
fork pitchfork\n\
forking furcation\n\
formaldehyde methanal\n\
formalin formol\n\
formalised formalistic formalized\n\
formalised formalized\n\
formality formalness\n\
formative plastic shaping\n\
former late previous\n\
formidability toughness\n\
formidable redoubtable unnerving\n\
formosa taiwan\n\
formulation preparation\n\
fort fortify\n\
fort fortress\n\
fort garrison\n\
forte loud\n\
forte metier speciality specialty\n\
fortification munition\n\
fortunate rosy\n\
fortune luck\n\
fosse moat\n\
fossilised fossilized ossified\n\
fossilist palaeontologist paleontologist\n\
foster surrogate\n\
foulness raininess\n\
foundry metalworks\n\
fount fountain\n\
fountain jet\n\
fountain outflow outpouring spring\n\
fountainhead headspring\n\
four foursome iv quadruplet quartet quatern quaternary quaternion quaternity tetrad\n\
four iv\n\
fourfold quadruple\n\
fourfold quadruple quadruplex quadruplicate\n\
fourth quarter quartern\n\
fowl poultry\n\
fr francium\n\
fractious recalcitrant refractory\n\
fractiousness unruliness wilfulness willfulness\n\
fradicin neobiotic neomycin\n\
fragment shard sherd\n\
fragmental fragmentary\n\
frailty vice\n\
francophil francophile\n\
frangipani frangipanni\n\
frank frankfurter hotdog weenie wiener wienerwurst\n\
frank postmark\n\
frankfort frankfurt\n\
frankincense olibanum thus\n\
frankness outspokenness\n\
frantic frenetic frenzied phrenetic\n\
frappe ice\n\
fraught pregnant\n\
fray frazzle\n\
freak monster monstrosity\n\
freckle lentigo\n\
freckled lentiginose lentiginous\n\
freedman freedwoman\n\
freehand freehanded\n\
freelance freelancer independent\n\
freelance mercenary\n\
freeman freewoman\n\
freemason mason\n\
freeze frost\n\
fremontia fremontodendron\n\
french gallic\n\
frenchman frenchwoman\n\
frenzied manic\n\
frequent haunt\n\
frequenter patron\n\
fresher freshman\n\
freshness novelty\n\
fretful querulous whiney whiny\n\
fretsaw jigsaw\n\
fretted interlaced latticed latticelike\n\
fretwork lattice latticework\n\
frey freyr\n\
freya freyja\n\
friar mendicant\n\
friction rubbing\n\
friend quaker\n\
friendless outcast\n\
frier fryer pullet\n\
friesian holstein\n\
frigg frigga\n\
frightened panicked panicky terrified\n\
frightened scared\n\
frightful terrible tremendous\n\
frigid frosty frozen glacial icy wintry\n\
frigidity frigidness\n\
frijol frijole\n\
frijolillo frijolito\n\
frill ruff\n\
frilled frilly ruffled\n\
fringe outskirt\n\
fringe periphery\n\
fringed laciniate\n\
fringepod lacepod\n\
fringy marginal\n\
friskiness frolicsomeness sportiveness\n\
frisky kittenish\n\
frivolity frivolousness\n\
frizzly frizzy kinky nappy\n\
frog gaul\n\
front presence\n\
frontal frontlet\n\
frost hoar hoarfrost rime\n\
frost ice\n\
frostiness hoariness\n\
frosting ice icing\n\
frostweed frostwort\n\
frosty nipping nippy\n\
frosty rimed rimy\n\
froward headstrong wilful willful\n\
frowsty fusty musty\n\
frowsy frowzy slovenly\n\
frozen rooted\n\
fructose laevulose levulose\n\
frugality frugalness\n\
fruit yield\n\
frustrating frustrative thwarting\n\
fruticose fruticulose shrubby\n\
frypan skillet\n\
fugaciousness fugacity\n\
fuji fujinoyama fujiyama\n\
fullness mellowness richness\n\
fullness voluminosity voluminousness\n\
fulsomeness oiliness oleaginousness smarminess unction unctuousness\n\
fulvicin griseofulvin\n\
fumble grope\n\
fume smoke\n\
fumeroot fumewort\n\
fumeroot fumewort fumitory\n\
fun playfulness\n\
function operate\n\
function purpose role use\n\
functional operable operational usable useable\n\
functional operative running working\n\
functionary official\n\
fundamental profound\n\
fundamental rudimentary underlying\n\
fundamentalist fundamentalistic\n\
funereal sepulchral\n\
fungal fungous\n\
fungoid funguslike\n\
funicle funiculus\n\
funka hosta\n\
funkaceae hostaceae\n\
fur pelt\n\
furan furane furfuran\n\
furfural furfuraldehyde\n\
furled rolled\n\
furnishing trappings\n\
furosemide lasix\n\
furred furry\n\
furrow groove rut\n\
furrow wrinkle\n\
furrowed rugged\n\
furtive sneak sneaky stealthy surreptitious\n\
furtiveness sneakiness stealthiness\n\
furze gorse whin\n\
fuscous taupe\n\
fuse fusee fuze fuzee primer priming\n\
fusee fuzee\n\
fusspot worrier worrywart\n\
fusty nonprogressive standpat unprogressive\n\
futile ineffectual meaningless otiose unavailing\n\
future next succeeding\n\
futurist futuristic\n\
fuzz hair tomentum\n\
fuzzed fuzzy\n\
ga gallium\n\
ga georgia\n\
ga tabun\n\
gabapentin neurontin\n\
gabon gabun\n\
gad gallivant\n\
gad spur\n\
gaddafi khadafy qaddafi qadhafi\n\
gadolinite ytterbite\n\
gadolinium gd\n\
gaea gaia ge\n\
gag muzzle\n\
gage gauge\n\
gagman gagster gagwriter\n\
gain profit\n\
gainful paid paying\n\
gainfulness lucrativeness profitability profitableness\n\
gaiseric genseric\n\
gaiter spat\n\
gal gallon\n\
galangal galingale\n\
galeras pasto\n\
galilaean galilean\n\
gallantry heroism valiance valiancy valor valorousness valour\n\
gallberry inkberry\n\
gallery heading\n\
gallery veranda verandah\n\
gallia gaul\n\
gallinule swamphen\n\
gallous gibbet\n\
gallus suspender\n\
galvanic voltaic\n\
galvanise galvanize\n\
galvaniser galvanizer\n\
galvaniser galvanizer inspirer\n\
gamboge lemon maize\n\
game gamey gamy gritty mettlesome spirited spunky\n\
gamey gamy high\n\
gamey gamy juicy naughty racy risque spicy\n\
gamin throwaway\n\
gaminess raciness ribaldry spiciness\n\
gammon ham jambon\n\
gamopetalous sympetalous\n\
ganapati ganesa ganesh ganesha\n\
gand gent ghent\n\
ganef ganof gonif goniff\n\
gangboard gangplank gangway\n\
gangling gangly lanky\n\
gangling gangly lanky rangy\n\
gangrene mortification necrosis sphacelus\n\
gangrenous mortified\n\
gangster mobster\n\
ganja marihuana marijuana\n\
ganoin ganoine\n\
gansu kansu\n\
gantanol sulfamethoxazole\n\
gantlet gauntlet\n\
gantrisin sulfisoxazole\n\
gantry gauntry\n\
gaolbird jailbird\n\
gaoler jailer jailor screw turnkey\n\
garamycin gentamicin\n\
garbage refuse scraps\n\
gardant guardant\n\
gardener nurseryman\n\
garget scoke\n\
gargle mouthwash\n\
gari mandioc mandioca manioc\n\
garishness gaudiness\n\
garner granary\n\
garotte garrote garrotte\n\
garotte garrote garrotte scrag\n\
garrulity garrulousness loquaciousness loquacity talkativeness\n\
gas gasolene gasoline petrol\n\
gasbag windbag\n\
gascogne gascony\n\
gash slash\n\
gasified vaporized vapourised volatilised volatilized\n\
gasmask respirator\n\
gasteromycete gastromycete\n\
gasteromycetes gastromycetes\n\
gasteropoda gastropoda\n\
gastric stomachal stomachic\n\
gastrointestinal gi\n\
gastronomic gastronomical\n\
gastropod univalve\n\
gat rod\n\
gather gathering\n\
gather pucker tuck\n\
gauche graceless unpolished\n\
gaucherie rusticity\n\
gauffer goffer\n\
gaumless gormless\n\
gaussmeter magnetometer\n\
gauze netting veiling\n\
gawkiness ungainliness\n\
gayal mithan\n\
gayfeather snakeroot\n\
gazebo summerhouse\n\
gbit gigabit\n\
gd soman\n\
ge germanium\n\
gean mazzard\n\
gear gearing geartrain train\n\
gearshift gearstick shifter\n\
geb keb\n\
gel gelatin\n\
gelatin gelatine\n\
gelatin jelly\n\
gelatinlike gelatinous jellylike\n\
gelatinousness glutinosity glutinousness\n\
gelignite gelly\n\
gem gemstone stone\n\
gem jewel\n\
gem muffin\n\
gem treasure\n\
gemfibrozil lopid\n\
geminate pair\n\
gemini twin\n\
gemini twins\n\
gemonil metharbital\n\
gemsbok gemsbuck\n\
gender sex sexuality\n\
genealogic genealogical\n\
generalised generalized\n\
generative procreative reproductive\n\
generative productive\n\
generosity generousness\n\
genetic genetical\n\
genetic genetical genic\n\
geneva geneve genf\n\
geneva hollands\n\
genial kind\n\
genial mental\n\
genip ginep mamoncillo\n\
genital venereal\n\
genitive possessive\n\
genitourinary gu\n\
genoa genova\n\
genoese genovese\n\
genotypic genotypical\n\
gentile goy\n\
gentile heathen infidel pagan\n\
gentleman valet\n\
gentlemanlike gentlemanly\n\
gentleness gradualness\n\
gentleness mildness softness\n\
genu knee\n\
genuflect kowtow\n\
genuine unfeigned\n\
geodesic geodesical geodetic\n\
geographic geographical\n\
geologic geological\n\
geometer geometrician\n\
geometric geometrical\n\
geomorphologic geomorphological morphologic morphological structural\n\
georgetown stabroek\n\
georgia sakartvelo\n\
geosphere lithosphere\n\
geothermal geothermic\n\
gerbil gerbille\n\
gerfalcon gyrfalcon\n\
geriatric gerontological\n\
geriatrician gerontologist\n\
germanic teutonic\n\
germinal originative seminal\n\
gerreidae gerridae\n\
gerridae gerrididae\n\
gestural nonverbal\n\
gestural sign signed\n\
getable gettable obtainable procurable\n\
getaway pickup\n\
getup outfit turnout\n\
ghanaian ghanese ghanian\n\
ghastliness grimness gruesomeness luridness\n\
ghastly grisly gruesome macabre\n\
ghillie gillie\n\
ghost ghostwriter\n\
ghostfish wrymouth\n\
ghoul graverobber\n\
ghoulish morbid\n\
gi gilbert\n\
giant heavyweight hulk whale\n\
giantism gigantism\n\
gib gibibyte gigabyte\n\
gibbose gibbous\n\
gibibit gibit\n\
giblet giblets\n\
giddiness silliness\n\
gifted talented\n\
gigantic mammoth\n\
giggler titterer\n\
gilded glossy meretricious specious\n\
gilded gold\n\
gilding gilt\n\
gill lamella\n\
gimp hobble limp\n\
gin noose snare\n\
ginger gingerroot\n\
ginger gingery\n\
ginger pep peppiness\n\
gingiva gum\n\
gingko ginkgo\n\
ginkgophytina ginkgopsida\n\
gipsy gypsy\n\
gipsy gypsy itinerant\n\
gipsywort gypsywort\n\
girandola girandole\n\
gird girdle\n\
girl girlfriend\n\
girlish schoolgirlish\n\
girondin girondist\n\
give spring springiness\n\
give yield\n\
given granted\n\
giza gizeh\n\
gizzard ventriculus\n\
glabella mesophyron\n\
glad gladiola gladiolus\n\
glad happy\n\
gladiator prizefighter\n\
gladstone portmanteau\n\
glamor glamour\n\
glamorous glamourous\n\
gland secreter secretor\n\
glareole pratincole\n\
glass glassful\n\
glass spyglass\n\
glassed glazed\n\
glasshouse greenhouse\n\
glassless unglazed\n\
glassware glasswork\n\
glassworker glazer glazier\n\
glasswort samphire\n\
glassy glazed\n\
glassy vitreous vitrified\n\
glazed shiny\n\
gleam gleaming glow lambency\n\
glean harvest reap\n\
glia neuroglia\n\
glib pat\n\
glibness slickness\n\
glider sailplane\n\
glipizide glucotrol\n\
glisten glister glitter scintillation sparkle\n\
glistening glossy lustrous sheeny shining shiny\n\
global globose globular orbicular spheric spherical\n\
global planetary world worldwide\n\
globin haematohiston hematohiston\n\
globosity globularness rotundity rotundness sphericalness sphericity\n\
glochid glochidium\n\
gloomful glooming gloomy sulky\n\
gloominess lugubriousness sadness\n\
glorious magnificent splendid\n\
glorious resplendent splendid splendiferous\n\
glory resplendence resplendency\n\
gloss semblance\n\
glossina tsetse tzetze\n\
glossy showy\n\
glove mitt\n\
glow glowing radiance\n\
glow incandescence\n\
glow luminescence\n\
glucophage metformin\n\
glue gum mucilage\n\
glue paste\n\
glued pasted\n\
gluey glutinous gummy mucilaginous pasty sticky viscid viscous\n\
glut oversupply surfeit\n\
glute gluteus\n\
glutted overfull\n\
glutton gourmand gourmandizer trencherman\n\
glutton wolverine\n\
glycerin glycerine glycerol\n\
glycerite glycerole\n\
glycerogel glycerogelatin\n\
glycerolise glycerolize\n\
glyoxaline imidazole iminazole\n\
glyptics lithoglyptics\n\
gm gram gramme\n\
gnarl knot\n\
gnarled gnarly knobbed knotted knotty\n\
gnawer rodent\n\
gnetophyta gnetophytina gnetopsida\n\
gnu wildebeest\n\
goad prod\n\
goalie goalkeeper goaltender netkeeper netminder\n\
goalless hitless pointless scoreless\n\
goat laughingstock stooge\n\
goatfish surmullet\n\
gob mariner seafarer seaman tar\n\
gobbler tom\n\
goblin hob hobgoblin\n\
goby gudgeon\n\
god idol\n\
godforsaken waste\n\
godless irreverent\n\
godlessness ungodliness\n\
godly reverent worshipful\n\
goering goring\n\
goeteborg goteborg gothenburg\n\
goethean goethian\n\
goethite gothite\n\
goffer gopher\n\
goldeneye whistler\n\
goldfinch yellowbird\n\
goldsmith goldworker\n\
golfer linksman\n\
golliwog golliwogg\n\
gomel homel homyel\n\
gomorrah gomorrha\n\
gonadotrophic gonadotropic\n\
gonadotrophin gonadotropin\n\
gondolier gondoliere\n\
goner toast\n\
goo gook goop guck gunk muck ooze slime sludge\n\
goodish goodly healthy hefty respectable sizable sizeable tidy\n\
goodwill grace\n\
gooey icky\n\
goofy silly wacky whacky zany\n\
goon hood hoodlum punk thug toughie\n\
gooney goonie goony\n\
goop max scoop soap\n\
gopher minnesotan\n\
gopher spermophile\n\
gore panel\n\
gorger scoffer\n\
gorgerin necking\n\
gorgonacea gorgoniacea\n\
gorki gorkiy gorky\n\
gorki gorky\n\
gossip gossiper gossipmonger newsmonger rumormonger rumourmonger\n\
gothic mediaeval medieval\n\
gouge rout\n\
goujon mudcat\n\
goulash gulyas\n\
governor regulator\n\
gown nightdress nightgown nightie\n\
gown robe\n\
gown scrubs\n\
grace gracility\n\
grace seemliness\n\
graceless ungraceful\n\
gracelessness ungracefulness\n\
gracilariidae gracillariidae\n\
gracile willowy\n\
grad grade\n\
gradational gradatory graduated\n\
graded ranked stratified\n\
gradient slope\n\
graduality gradualness\n\
graduate postgraduate\n\
graecophile graecophilic philhellene philhellenic\n\
graecophile philhellene philhellenist\n\
graffiti graffito\n\
graft transplant\n\
grail sangraal\n\
grain ingrain\n\
grain texture\n\
graining woodgraining\n\
grama gramma\n\
graminaceae gramineae poaceae\n\
grammarian syntactician\n\
grammatic grammatical\n\
gramps grandad granddad granddaddy grandfather grandpa\n\
grampus killer orca\n\
gran grandma grandmother grannie granny nan nanna\n\
grandeur magnanimousness nobility nobleness\n\
grandiloquent magniloquent tall\n\
grandiloquent overblown pompous pontifical portentous\n\
grandiose hifalutin highfalutin highfaluting\n\
grandness impressiveness magnificence richness\n\
granitelike granitic rocklike stony\n\
grape grapeshot\n\
grape grapevine\n\
grapey grapy\n\
graphic graphical\n\
graphic lifelike pictorial vivid\n\
graphite plumbago\n\
grapnel grapple grappler\n\
grappler matman wrestler\n\
grass supergrass\n\
grasshopper hopper\n\
grate grating\n\
grate grind\n\
grateful thankful\n\
graticule reticle reticule\n\
grating gravelly rasping raspy scratchy\n\
gratuitous needless\n\
gravelly pebbly shingly\n\
graven sculpted sculptured\n\
graveness gravity soberness sobriety somberness sombreness\n\
graver pointel pointrel\n\
gravestone headstone tombstone\n\
gravimeter hydrometer\n\
gravimetric hydrometric\n\
gravitation gravity\n\
gravitational gravitative\n\
gravure heliogravure photogravure\n\
grayback greyback\n\
grayback greyback knot\n\
graybeard greybeard methuselah\n\
grayhen greyhen\n\
grayish grey greyish\n\
graylag greylag\n\
grayness grey greyness\n\
graze pasture\n\
graze rake\n\
greased lubricated\n\
greaser taco wetback\n\
greasiness oiliness oleaginousness\n\
greasy oily\n\
greasy oily oleaginous sebaceous\n\
great outstanding\n\
greatcoat overcoat topcoat\n\
greatest sterling superlative\n\
greathearted magnanimous\n\
greatness illustriousness\n\
greave jambeau\n\
grecian greek hellenic\n\
greediness hoggishness piggishness\n\
greediness rapaciousness voraciousness\n\
greegree grigri\n\
greek hellene\n\
greenbelt greenway\n\
greenery verdure\n\
greening rejuvenation\n\
greenland gronland\n\
greenness verdancy verdure\n\
greenness viridity\n\
greensward sod sward turf\n\
greeter saluter welcomer\n\
gregory hildebrand\n\
grenadier rattail\n\
grey grizzly hoar hoary\n\
greyback johnny reb rebel\n\
grid gridiron\n\
griever lamenter mourner sorrower\n\
grievous heartbreaking heartrending\n\
grievous weighty\n\
griffin griffon gryphon\n\
grill grille grillwork\n\
grill grillroom\n\
grille lattice wicket\n\
grillwork wirework\n\
grinder mill\n\
grinder molar\n\
grit gritrock gritstone\n\
grizzly silvertip\n\
grocery market\n\
groin inguen\n\
groom hostler ostler stableboy stableman\n\
groove vallecula\n\
groovy swagger\n\
grosbeak grossbeak\n\
grot grotto\n\
grotesque monstrous\n\
grotesqueness grotesquerie grotesquery\n\
groucho marx\n\
groundbreaker innovator pioneer trailblazer\n\
groundbreaking innovational innovative\n\
groundhog woodchuck\n\
groundkeeper groundskeeper groundsman\n\
groundlessness idleness\n\
group grouping\n\
group radical\n\
grouped sorted\n\
grove orchard plantation woodlet\n\
grozny groznyy\n\
grudging niggardly scrimy\n\
gruff hoarse husky\n\
gruffness hoarseness huskiness\n\
grugru macamba\n\
grumbling rumbling\n\
grundyism primness prudery prudishness\n\
grunter hog pig squealer\n\
gu guam\n\
guacharo oilbird\n\
guaiac guaiacum\n\
guanabana soursop\n\
guanabenz wytensin\n\
guangdong kwangtung\n\
guarantor surety warranter warrantor\n\
guard safety\n\
guarded restrained\n\
guarneri guarnerius guarnieri\n\
guenevere guinevere\n\
guerilla guerrilla insurgent\n\
guest invitee\n\
guileless transparent\n\
guilty hangdog shamed shamefaced\n\
guise pretence pretense pretext\n\
gujarat gujerat\n\
gujarati gujerati\n\
gulfweed sargasso sargassum\n\
gull seagull\n\
gulper guzzler\n\
gum gumwood\n\
gumbo okra\n\
gummed gummy\n\
gumshield mouthpiece\n\
gumweed rosinweed tarweed\n\
gun gunman\n\
gun gunman gunslinger hitman shooter torpedo triggerman\n\
guncotton nitrocellulose nitrocotton\n\
gunnel gunwale\n\
gunpowder powder\n\
gush jet\n\
gush spirt spout spurt\n\
gushing pouring\n\
gusset inset\n\
gusset voider\n\
gustative gustatorial gustatory\n\
gusty puffy\n\
gutless spineless\n\
gutsiness pluckiness\n\
gutsy plucky\n\
gutter trough\n\
gwynn gynne gywn\n\
gybe jib jibe\n\
gym gymnasium\n\
gymnomycota myxomycota\n\
gymnospermae gymnospermophyta\n\
gynaecological gynecologic gynecological\n\
gynaecologist gynecologist\n\
gynandromorphic gynandromorphous\n\
gyrate reel spin\n\
gyro gyroscope\n\
gyrostabiliser gyrostabilizer\n\
h2o water\n\
habitability habitableness\n\
habitable inhabitable\n\
hack jade nag\n\
hackamore halter\n\
hackberry sugarberry\n\
hackelia lappula\n\
hackle hatchel heckle\n\
hackmatack tacamahac\n\
hadean plutonian tartarean\n\
hadji haji hajji\n\
hadrosaur hadrosaurus\n\
haem haemitin hematin heme protoheme\n\
haemagglutinate hemagglutinate\n\
haemal haematal hemal hematal\n\
haematic haemic hematic hemic\n\
haematinic hematinic\n\
haematite hematite\n\
haematocrit hematocrit\n\
haematogenic haematopoietic haemopoietic hematogenic hematopoietic hemopoietic\n\
haematological hematologic hematological\n\
haematologist hematologist\n\
haematoxylon haematoxylum\n\
haemoglobin hb hemoglobin\n\
haemolytic hemolytic\n\
haemophilic hemophilic\n\
haemoprotein hemoprotein\n\
haemorrhagic hemorrhagic\n\
haemosiderin hemosiderin\n\
haemostat hemostat\n\
hafnium hf\n\
haft helve\n\
hag hagfish\n\
hagiographer hagiographist hagiologist\n\
hagridden tormented\n\
haick haik\n\
haifa hefa\n\
hair haircloth\n\
hair hairsbreadth whisker\n\
hair pilus\n\
hairball trichobezoar\n\
hairdresser hairstylist styler stylist\n\
haired hairy hirsute\n\
hairiness pilosity\n\
hairpiece postiche\n\
haiti hayti hispaniola\n\
hakeem hakim\n\
halcion triazolam\n\
halcyon prosperous\n\
haldol haloperidol\n\
hale whole\n\
haler heller\n\
halfhearted lukewarm tepid\n\
halftone photoengraving\n\
halfway middle midway\n\
halibut holibut\n\
hall hallway\n\
hall manse mansion residence\n\
halliard halyard\n\
hallowed sacred\n\
halm haulm\n\
halobacter halobacteria halobacterium\n\
halophil halophile\n\
haloragaceae haloragidaceae\n\
halter hemp\n\
hamelin hameln\n\
hamlet village\n\
hammer mallet\n\
hammer malleus\n\
hammock hillock hummock knoll mound\n\
hammurabi hammurapi\n\
hand manus mitt paw\n\
handbag pocketbook purse\n\
handbasin lavabo washbasin washbowl\n\
handbreadth handsbreadth\n\
handcraft handicraft handiwork handwork\n\
handedness laterality\n\
handful smattering\n\
handgrip handle\n\
handgun pistol\n\
handkerchief hankey hankie hanky\n\
handle manage wield\n\
handle palm\n\
handmaid handmaiden\n\
handsewn handstitched\n\
handstamp rubberstamp\n\
hangchow hangzhou\n\
hangout haunt repair resort\n\
hangover holdover\n\
hannover hanover\n\
haoma soma\n\
haphazard slapdash slipshod sloppy\n\
haphazardness noise randomness stochasticity\n\
hapless misfortunate pathetic piteous pitiable pitiful poor\n\
haploid haploidic monoploid\n\
haptic tactile tactual\n\
harare salisbury\n\
harasser harrier\n\
harbor harbour\n\
harbor harbour haven seaport\n\
harborage harbourage\n\
hardback hardbacked hardbound hardcover\n\
hardback hardcover\n\
hardened tempered toughened treated\n\
hardheaded mulish\n\
hardheaded practical pragmatic\n\
hardhearted heartless\n\
hardhearted stonyhearted unfeeling\n\
hardiness lustiness robustness\n\
hardness harshness inclemency rigor rigorousness rigour rigourousness severeness severity stiffness\n\
hardness ruggedness\n\
hardware ironware\n\
hardwareman ironmonger\n\
hardworking industrious tireless untiring\n\
hardy stalwart stout sturdy\n\
hare rabbit\n\
harebrained insane mad\n\
hareem harem seraglio serail\n\
harijan untouchable\n\
harmfulness injuriousness\n\
harmfulness noisomeness noxiousness\n\
harmonic sympathetic\n\
harmonica harp\n\
harmonious proportionate symmetrical\n\
harmoniousness harmony\n\
harmoniser harmonizer\n\
harmonium organ\n\
harness tackle\n\
harper harpist\n\
harpo marx\n\
harpooneer harpooner\n\
harpy hellcat vixen\n\
harshness roughness\n\
hart stag\n\
harvester reaper\n\
haschisch hash hasheesh hashish\n\
hassium hs\n\
hassock ottoman pouf pouffe\n\
haste hastiness hurriedness hurry precipitation\n\
hasten hie hotfoot race rush speed\n\
hasty headlong\n\
hasty overhasty precipitant precipitate precipitous\n\
hatch hatchback liftgate\n\
hatchel heckle\n\
hatchet tomahawk\n\
hatchway opening scuttle\n\
hatefulness objectionableness obnoxiousness\n\
hatmaker hatter milliner modiste\n\
hauler haulier\n\
haunt stalk\n\
haunted obsessed preoccupied\n\
haunting persistent\n\
hausa haussa\n\
hautbois hautboy oboe\n\
haven oasis\n\
haw hawthorn\n\
hawaii hi\n\
hawk mortarboard\n\
hawkbill hawksbill\n\
hawker packman peddler pedlar pitchman\n\
hawkins hawkyns\n\
hawkish militant warlike\n\
hawkmoth sphingid\n\
hawse hawsehole hawsepipe\n\
hayfield meadow\n\
hayloft haymow mow\n\
hayrack hayrig\n\
hazardous risky\n\
hazel hazelnut\n\
haziness mistiness steaminess vaporousness vapourousness\n\
hcfc hydrochlorofluorocarbon\n\
he helium\n\
headdress headgear\n\
header lintel\n\
headfirst headlong\n\
headfish mola sunfish\n\
headlamp headlight\n\
headliner star\n\
headman headsman\n\
headmaster schoolmaster\n\
headpiece headstall\n\
headpin kingpin\n\
headquarters hq\n\
headstone keystone\n\
heady intoxicating\n\
heady judicious wise\n\
healer therapist\n\
healthful sanitary\n\
healthy intelligent levelheaded\n\
healthy salubrious\n\
heap stack\n\
heart mettle nerve spunk\n\
heart pump ticker\n\
heart spirit\n\
heartening inspiriting\n\
heartiness wholeheartedness\n\
hearty lusty\n\
hearty satisfying\n\
heat heating\n\
heat hotness\n\
heat passion warmth\n\
heated het\n\
heater warmer\n\
heath heathland\n\
heave heft\n\
heavenward skyward\n\
heaviness ponderousness\n\
heaviness thickness\n\
heaviness weightiness\n\
heavyset stocky thickset\n\
hebdomadal hebdomadary weekly\n\
hebei hopeh hopei\n\
hebraic hebraical hebrew\n\
hebrew israelite jew\n\
hectogram hg\n\
hectograph heliotype\n\
hectoliter hectolitre hl\n\
hectometer hectometre hm\n\
hedge hedgerow\n\
hedgehog porcupine\n\
hedjaz hejaz hijaz\n\
hedonist pagan\n\
heedfulness mindfulness\n\
heedless reckless\n\
heedless unheeding\n\
heedlessness inadvertence inadvertency unmindfulness\n\
heedlessness mindlessness rashness\n\
heel list\n\
heft heftiness massiveness ponderosity ponderousness\n\
height stature\n\
height tallness\n\
heights high\n\
heimdal heimdall heimdallr\n\
heir heritor inheritor\n\
heir successor\n\
heiress inheritress inheritrix\n\
hel hela\n\
heliac heliacal\n\
helianthemum sunrose\n\
helianthus sunflower\n\
heliopsis oxeye\n\
hell hellhole inferno\n\
hellenic hellenistic hellenistical\n\
helleri swordtail topminnow\n\
helmetflower monkshood\n\
helmetflower skullcap\n\
helmsman steerer steersman\n\
helot serf villein\n\
helpfulness kindliness\n\
helping portion serving\n\
helpless incapacitated\n\
helplessness impuissance weakness\n\
helpmate helpmeet\n\
helsingfors helsinki\n\
helxine soleirolia\n\
hemicycle semicircle\n\
hemiepiphyte semiepiphyte\n\
hemimetabolic hemimetabolous hemimetamorphic hemimetamorphous\n\
hemin protohemin\n\
heming hemminge\n\
hemiparasite semiparasite\n\
hemostatic styptic\n\
hemstitch hemstitching\n\
hep hip\n\
heparin liquaemin\n\
hepatic liverwort\n\
hepatica liverleaf\n\
hepaticae hepaticopsida\n\
hepatoflavin lactoflavin ovoflavin riboflavin\n\
hephaestus hephaistos\n\
heptad septenary septet seven sevener vii\n\
hera here\n\
herald trumpeter\n\
heraldic heraldist\n\
herbage pasturage\n\
herbicide weedkiller\n\
herculean powerful\n\
herculius maximian\n\
hereford whiteface\n\
heretic misbeliever\n\
heritable inheritable\n\
heritage inheritance\n\
heritiera terrietia\n\
hermaphrodite hermaphroditic\n\
hermit recluse solitudinarian troglodyte\n\
hero heron\n\
heroic heroical\n\
herrerasaur herrerasaurus\n\
hertha nerthus\n\
hesitant hesitating\n\
hesitater hesitator vacillator waverer\n\
hesperian occidental\n\
hesperus vesper\n\
hessian jackboot wellington\n\
heterocycle heterocyclic\n\
heterodoxy unorthodoxy\n\
heterogeneity heterogeneousness\n\
heterogeneous heterogenous\n\
heterogenesis xenogenesis\n\
heterograft xenograft\n\
heteroicous polygamous polyoicous\n\
heterologic heterological heterologous\n\
heterometabolic heterometabolous\n\
hex hexadecimal\n\
hexad sestet sextet sextuplet sise six sixer vi\n\
hexagonal hexangular\n\
hexapoda insecta\n\
hexed jinxed\n\
hexenbesen staghead\n\
hfc hydrofluorocarbon\n\
hg hydrargyrum mercury quicksilver\n\
hibernia ireland\n\
hide pelt\n\
hideaway retreat\n\
hidebound traditionalist\n\
hideous horrid horrific outrageous\n\
hideous repulsive\n\
hierarchal hierarchic hierarchical\n\
hieratic hieratical priestly sacerdotal\n\
hieroglyphic hieroglyphical\n\
hieronymus jerome\n\
high mellow\n\
highboy tallboy\n\
highbrow highbrowed\n\
highflier highflyer\n\
highjacker highwayman hijacker\n\
highjacker hijacker\n\
highland upland\n\
highlight highlighting\n\
highness loftiness\n\
hiker tramper\n\
hilarious screaming uproarious\n\
hill mound\n\
hilum hilus\n\
himalaya himalayas\n\
hind hinder\n\
hindbrain rhombencephalon\n\
hindi hindoo hindu\n\
hindoo hindu\n\
hindoo hindu hindustani\n\
hint jot mite pinch soupcon speck tinge touch\n\
hint suggestion tint\n\
hip pelvis\n\
hip rosehip\n\
hippie hippy hipster\n\
hippo hippopotamus\n\
hireling pensionary\n\
hirsuteness hirsutism\n\
hispanic latino\n\
hiss whoosh\n\
histologic histological\n\
historian historiographer\n\
historic historical\n\
histrionic melodramatic\n\
hit strike\n\
hitchhike thumb\n\
hitter striker\n\
hmong miao\n\
ho holmium\n\
hoactzin hoatzin stinkbird\n\
hoary rusty\n\
hoaxer prankster tricker trickster\n\
hobble hopple\n\
hobbler limper\n\
hobby hobbyhorse\n\
hock rhenish\n\
hoder hodr hodur hoth hothr\n\
hodometer mileometer milometer odometer\n\
hog hogg hogget\n\
hog pig\n\
hogback horseback\n\
hogfish pigfish\n\
hoggish piggish piggy porcine swinish\n\
hoist wind\n\
hoka hokan\n\
hole hollow\n\
holey porous\n\
holidaymaker tourer tourist\n\
holiness sanctitude sanctity\n\
holland nederland netherlands\n\
holler hollow\n\
holloware hollowware\n\
holocephalan holocephalian\n\
hologram holograph\n\
holographic holographical\n\
holometabola metabola\n\
holometabolic holometabolous\n\
holy sanctum\n\
homebound housebound\n\
homebuilder housebuilder\n\
homeless stateless\n\
homelike homely homey homy\n\
homeliness plainness\n\
homemaker housewife\n\
homeopath homoeopath\n\
homeotherm homoiotherm homotherm\n\
homeothermic homoiothermic homothermic\n\
homeowner householder\n\
homer kor\n\
homespun nubbly nubby slubbed tweedy\n\
homesteader nester squatter\n\
homicidal murderous\n\
homiletic homiletical\n\
hominian hominid\n\
hommos hoummos hummus humous humus\n\
homo homophile homosexual\n\
homo human\n\
homochromatic monochromatic\n\
homocyclic isocyclic\n\
homogeneity homogeneousness\n\
homogeneous homogenous\n\
homogenised homogenized\n\
homologic homological\n\
homomorphism homomorphy\n\
homonymic homonymous\n\
homophile queer\n\
homostyled homostylic homostylous\n\
homunculus manikin mannikin\n\
hondo honshu\n\
honest honorable\n\
honestness honesty\n\
honesty satinpod\n\
honeyed honied syrupy\n\
honeymooner newlywed\n\
honkey honkie honky whitey\n\
honor honour\n\
honor honour pureness purity\n\
honorable honourable\n\
honorableness honourableness\n\
hooch hootch\n\
hoofed hooved ungulate ungulated\n\
hoofer stepper\n\
hooked hooklike\n\
hooks maulers\n\
hoop ring\n\
hoop wicket\n\
hoopoe hoopoo\n\
hoosegow hoosgow\n\
hoosier indianan\n\
hooter horn\n\
hooter owl\n\
hoover vacuum\n\
hop hops\n\
hop skip\n\
hopeful promising\n\
hopsack hopsacking\n\
horizon purview view\n\
horizon skyline\n\
horn tusk\n\
hornfels hornstone\n\
hornpipe pibgorn stockhorn\n\
hornpout pout\n\
horologe timekeeper timepiece\n\
horologer horologist watchmaker\n\
horse knight\n\
horseflesh horsemeat\n\
horsepower hp\n\
horseshoe shoe\n\
horseweed richweed stoneroot\n\
horticulturist plantsman\n\
hose hosepipe\n\
hose hosiery\n\
hospital infirmary\n\
host server\n\
hostage surety\n\
hosteller hotelier hotelkeeper hotelman\n\
hostess stewardess\n\
hostile uncongenial unfriendly\n\
hot live\n\
hot raging\n\
hot spicy\n\
hotness pepperiness\n\
hotspur percy\n\
houdah howdah\n\
houhere lacebark ribbonwood\n\
hound hunt\n\
hour minute\n\
houri nymph\n\
house mansion sign\n\
house theater theatre\n\
housebreaker housewrecker\n\
housecoat neglige negligee peignoir wrapper\n\
houseman intern interne\n\
housing lodging\n\
hovel hut hutch shack shanty\n\
hover levitate\n\
howitzer mortar\n\
hoyden romp tomboy\n\
hoydenish tomboyish\n\
hoydenism tomboyishness\n\
hrolf rolf rollo\n\
hualapai hualpai walapai\n\
huarache huaraches\n\
hubby husband\n\
huck huckaback\n\
huffish sulky\n\
huffy mad sore\n\
huitre oyster\n\
hulking hulky\n\
humane humanist humanistic\n\
humanist humanistic\n\
humanist humanistic humanitarian\n\
humanist humanitarian\n\
humanitarian improver\n\
humanity humankind humans mankind world\n\
humanity humanness manhood\n\
humble lowly menial\n\
humble lowly modest\n\
humbleness humility\n\
humdrum monotonous\n\
humdrum monotony sameness\n\
humor humour\n\
humorist humourist\n\
humorless humourless unhumorous\n\
humorous humourous\n\
humorousness jocoseness jocosity merriness\n\
hump hunch\n\
hungarian magyar\n\
hungary magyarorszag\n\
hunger hungriness thirst thirstiness\n\
hunk lump\n\
hunter huntsman\n\
hunter orion\n\
hurl hurtle\n\
hurl hurtle lunge\n\
hurler pitcher twirler\n\
hurry speed zip\n\
hurrying scurrying\n\
hurt weakened\n\
hurt wounded\n\
hus huss\n\
husain husayn hussein\n\
husain husayn hussein saddam\n\
hush stillness\n\
hushed muted subdued\n\
huskiness ruggedness toughness\n\
hustler operator\n\
huxleian huxleyan\n\
hyacinth jacinth\n\
hyaena hyena\n\
hyalin hyaline\n\
hyaline hyaloid\n\
hyaluronidase hyazyme\n\
hybrid intercrossed\n\
hydnocarpus taraktagenos taraktogenos\n\
hydra snake\n\
hydrant tap\n\
hydrated hydrous\n\
hydrocharidaceae hydrocharitaceae\n\
hydrofoil hydroplane\n\
hydrographic hydrographical\n\
hydroid hydrozoan\n\
hydroplane seaplane\n\
hydroxybenzene oxybenzene phenol\n\
hydroxychloroquine plaquenil\n\
hydroxytetracycline oxytetracycline terramycin\n\
hygienic hygienical\n\
hygienise hygienize sanitise sanitize\n\
hymen maidenhead\n\
hymenopter hymenopteran hymenopteron\n\
hymie kike sheeny yid\n\
hyoscine scopolamine\n\
hypaethral hypethral\n\
hyperactive overactive\n\
hyperbolic inflated\n\
hypercritical overcritical\n\
hypericales parietales\n\
hypermetropic hyperopic\n\
hyperoartia petromyzoniformes\n\
hyperodontidae ziphiidae\n\
hyperotreta myxiniformes myxinoidea myxinoidei\n\
hypnagogic hypnogogic somniferous somnific soporiferous soporific\n\
hypnotic mesmeric mesmerizing spellbinding\n\
hypnotic soporific\n\
hypnotiser hypnotist hypnotizer mesmerist mesmerizer\n\
hypo hypodermic\n\
hypoactive underactive\n\
hypochaeris hypochoeris\n\
hypochondriac hypochondriacal\n\
hypodermatidae oestridae\n\
hypodermic subcutaneous\n\
hypoglycaemic hypoglycemic\n\
hypognathous prognathic prognathous\n\
hypophyseal hypophysial\n\
hypophysectomised hypophysectomized\n\
hypophysis pituitary\n\
hypovolaemic hypovolemic\n\
hysteric hysterical\n\
hytrin terazosin\n\
ia iowa\n\
iceboat icebreaker\n\
iceboat scooter\n\
icebox refrigerator\n\
ichorous sanious\n\
icon ikon\n\
icon ikon image picture\n\
icsh lh\n\
ictal ictic\n\
icteric jaundiced\n\
id idaho\n\
ideal idealistic\n\
idealised idealized\n\
idealogue theoretician theoriser theorist theorizer\n\
identical indistinguishable\n\
identical monovular\n\
identical selfsame very\n\
identical superposable\n\
identicalness identity indistinguishability\n\
identity individuality\n\
ideologic ideological\n\
ideologist ideologue\n\
idiomatic idiomatical\n\
idiotic imbecile imbecilic\n\
idocrase vesuvian vesuvianite\n\
idolater idoliser idolizer\n\
idoliser idolizer\n\
idun ithunn\n\
ig immunoglobulin\n\
igloo iglu\n\
igneous pyrogenic pyrogenous\n\
igniter ignitor lighter\n\
ignobility ignobleness\n\
ignoble ungentle untitled\n\
ignorant illiterate\n\
ignorant nescient unlearned unlettered\n\
ignorant unknowing unknowledgeable unwitting\n\
ignored neglected unheeded\n\
iguania iguanidae\n\
iguassu iguazu\n\
ii two\n\
iii three\n\
il illinois\n\
ilion ilium troy\n\
ill inauspicious ominous\n\
illative inferential\n\
illegitimate illicit outlaw outlawed unlawful\n\
illiberal intolerant\n\
illimitable limitless measureless unmeasured\n\
illiterate nonreader\n\
illogic illogicality illogicalness inconsequence\n\
illogical unlogical\n\
illuminance illumination\n\
illuminated lighted lit\n\
illumination miniature\n\
illusional illusionary\n\
illusionist seer visionary\n\
illusive illusory\n\
image persona\n\
imaginative inventive\n\
imam imaum\n\
imavate imipramine tofranil\n\
imbalanced unbalanced\n\
imbauba trumpetwood\n\
imbricate imbricated\n\
imbrication lapping overlapping\n\
imitator impersonator\n\
immaculate speckless spic spick spotless\n\
immaculate undefiled\n\
immanent subjective\n\
immaterial incorporeal\n\
immaterial nonmaterial\n\
immateriality incorporeality\n\
immature unfledged\n\
immature unripe unripened\n\
immeasurable immensurable unmeasurable\n\
immeasurable incomputable inestimable\n\
immediacy immediateness\n\
immediacy immediateness instancy instantaneousness\n\
immediate prompt straightaway\n\
immerse plunge\n\
imminent impendent impending\n\
immiscible unmixable\n\
immobilise immobilize trap\n\
immoderateness immoderation\n\
immotile nonmotile\n\
immovability immovableness\n\
immovable immoveable stabile unmovable\n\
immune resistant\n\
immunised immunized vaccinated\n\
immunologic immunological\n\
immunosuppressant immunosuppressive immunosuppressor\n\
imp monkey rapscallion rascal scalawag scallywag scamp\n\
impact wallop\n\
impacted wedged\n\
impale stake\n\
impalpability intangibility intangibleness\n\
impalpable intangible\n\
impartial unprejudiced\n\
impassable unpassable\n\
impassive stolid\n\
impatient raring\n\
impeccant sinless\n\
impecunious penniless penurious pinched\n\
impedance resistance resistivity\n\
impede jam obstruct obturate occlude\n\
impediment impedimenta obstructer obstruction obstructor\n\
impel propel\n\
impenetrability impenetrableness\n\
impenetrability imperviousness\n\
impenitence impenitency\n\
impenitent unremorseful unrepentant\n\
imperativeness instancy\n\
imperceptible unperceivable\n\
imperial majestic purple regal royal\n\
imperialist imperialistic\n\
imperishability imperishableness imperishingness\n\
impermanence impermanency\n\
impermanent temporary\n\
impermeability impermeableness\n\
impersonal neutral\n\
impertinent impudent overbold sassy saucy smart wise\n\
impertinent irreverent pert saucy\n\
imperturbable unflappable\n\
imperviable impervious\n\
impetuosity impetuousness\n\
impetus impulsion\n\
impiety impiousness\n\
impious undutiful\n\
impishness mischievousness puckishness whimsicality\n\
implanted ingrained planted\n\
implausibility implausibleness\n\
implemental instrumental subservient\n\
implicative suggestive\n\
implicit inexplicit\n\
implicit unquestioning\n\
import importation\n\
import importee\n\
important significant\n\
impossible inconceivable unimaginable\n\
impossible unacceptable\n\
impost springer\n\
impotence impotency powerlessness\n\
impracticability impracticableness\n\
impracticable infeasible unfeasible unworkable\n\
impreciseness imprecision\n\
impregnable inexpugnable\n\
impregnable inviolable unassailable unattackable\n\
impresario promoter showman\n\
impress imprint\n\
impress shanghai\n\
impressible impressionable waxy\n\
impressionist impressionistic\n\
impressive telling\n\
improbability improbableness\n\
improbable marvellous marvelous tall\n\
improbable unbelievable unconvincing unlikely\n\
improbable unlikely\n\
improper unconventional unlawful\n\
improperness impropriety\n\
improvidence shortsightedness\n\
improvident shortsighted\n\
improving up\n\
improvised makeshift\n\
impudent insolent\n\
impulse momentum\n\
impulsive unprompted\n\
impure unclean\n\
in inch\n\
in indiana\n\
in indium\n\
inability unfitness\n\
inaccessibility unavailability\n\
inaccessible unaccessible\n\
inaccessible unobtainable unprocurable untouchable\n\
inactive motionless static\n\
inactive nonoperational\n\
inactive passive\n\
inactiveness inactivity inertia\n\
inadequacy inadequateness\n\
inadequate jejune poor\n\
inadequate unequal\n\
inadvisable unadvisable\n\
inaesthetic unaesthetic\n\
inalienable unalienable\n\
inalienable unforfeitable\n\
inalterable unalterable\n\
inanimate nonliving\n\
inanimateness lifelessness\n\
inanition lassitude lethargy slackness\n\
inanity mindlessness pointlessness senselessness vacuity\n\
inapplicable unsuitable\n\
inappositeness inaptness\n\
inappropriate unfitting\n\
inappropriateness unworthiness\n\
inappropriateness wrongness\n\
inarguable unarguable\n\
inarticulate unarticulate\n\
inartistic unartistic\n\
inattentive neglectful\n\
inaudibility inaudibleness\n\
inaudible unhearable\n\
inaugural initiative initiatory maiden\n\
inauspicious unfortunate\n\
inauspiciousness unpropitiousness\n\
inauthentic spurious unauthentic\n\
inbound inward\n\
inca incan inka\n\
incalculable untold\n\
incapability incapableness\n\
incapable incompetent\n\
incaution incautiousness\n\
incendiary incitive inflammatory instigative seditious\n\
incensed indignant outraged umbrageous\n\
inceptive inchoative\n\
incertain uncertain unsure\n\
inchoate incipient\n\
inchworm looper\n\
incident incidental\n\
incisiveness trenchancy\n\
incisura incisure\n\
incite prod\n\
inclination leaning list tilt\n\
inclination tendency\n\
incline ramp\n\
incline side slope\n\
incline slope\n\
incognizable incognoscible\n\
incognizant unaware\n\
incombustible noncombustible\n\
incommunicative uncommunicative\n\
incomparable uncomparable\n\
incompetence incompetency\n\
incompetent unqualified\n\
incompetent unskilled\n\
incomplete uncomplete\n\
incomplete uncompleted\n\
incomprehensible inexplicable\n\
incomprehensible uncomprehensible\n\
incomprehensive noncomprehensive\n\
incongruity incongruousness\n\
inconsequent inconsequential\n\
inconsiderate unconsidered\n\
inconsiderateness inconsideration thoughtlessness\n\
inconspicuous invisible\n\
incontestable incontestible\n\
incontestable indisputable undisputable\n\
incontrovertibility incontrovertibleness positiveness positivity\n\
incontrovertible irrefutable\n\
inconvenience troublesomeness worriment\n\
inconvertible unconvertible unexchangeable\n\
inconvertible untransmutable\n\
incorporate incorporated integrated merged unified\n\
incorrectness wrongness\n\
incorrupt undecomposed\n\
incorruption incorruptness\n\
increase increment\n\
incredibility incredibleness\n\
incredible unbelievable\n\
inculpative inculpatory\n\
incumbent officeholder\n\
incurability incurableness\n\
incursive invading invasive\n\
incurvate incurved\n\
indapamide lozal\n\
indecent indecorous unbecoming uncomely unseemly untoward\n\
indecipherable unclear undecipherable unreadable\n\
indecision indecisiveness\n\
indecorous indelicate\n\
indecorousness indecorum\n\
indefatigability indefatigableness tirelessness\n\
indefatigable tireless unflagging unwearying\n\
indefensible insupportable unjustifiable unwarrantable unwarranted\n\
indefensible untenable\n\
indefinable indescribable ineffable unspeakable untellable unutterable\n\
indefinable undefinable\n\
indefiniteness indefinity indeterminacy indeterminateness indetermination\n\
indelible unerasable\n\
indentation indenture\n\
independent main\n\
inderal propanolol\n\
indeterminable undeterminable\n\
indeterminate undetermined\n\
indicative indicatory revelatory significative suggestive\n\
indifference nonchalance unconcern\n\
indigestibility indigestibleness\n\
indiscernible insensible undetectable\n\
indiscipline undiscipline\n\
indiscretion injudiciousness\n\
indiscriminating undiscriminating\n\
indispensability indispensableness vitalness\n\
indisputability indubitability unquestionability unquestionableness\n\
indisputable sure\n\
indissoluble insoluble\n\
indistinguishable undistinguishable\n\
individual mortal person somebody someone soul\n\
individual private\n\
individual single\n\
individualised individualized personalised personalized\n\
individualism individuality individuation\n\
individualist individualistic\n\
indocile uncontrollable ungovernable unruly\n\
indocin indomethacin\n\
indolence laziness\n\
indomitability invincibility\n\
indomitable unsubduable\n\
indrawn withdrawn\n\
indri indris\n\
inducer persuader\n\
inducive inductive\n\
inductance induction\n\
inductance inductor\n\
indulgence lenience leniency\n\
indulgent lenient\n\
indument indumentum\n\
industrialised industrialized\n\
inedible uneatable\n\
ineffable unnameable unspeakable unutterable\n\
ineffective ineffectual unable\n\
ineffective ineffectual uneffective\n\
ineffective inefficient\n\
ineffectiveness ineffectuality ineffectualness\n\
inefficaciousness inefficacy\n\
inelaborate unelaborate\n\
ineluctability unavoidability\n\
ineluctable inescapable unavoidable\n\
inept tactless\n\
ineptitude worthlessness\n\
ineptness unsuitability unsuitableness\n\
inequitable unjust\n\
inequity unfairness\n\
inerrable inerrant unerring\n\
inert neutral\n\
inert sluggish soggy torpid\n\
inessential nonessential\n\
inessential unessential\n\
inevitability inevitableness\n\
inexactitude inexactness\n\
inexcusable unforgivable\n\
inexhaustible unlimited\n\
inexorability inexorableness relentlessness\n\
inexorable relentless unappeasable unforgiving unrelenting\n\
inexpedience inexpediency\n\
inexpedient unwise\n\
inexperienced inexperient\n\
inexpressible unexpressible\n\
inexpungeable inexpungible\n\
inextensible nonextensile nonprotractile\n\
inexterminable inextirpable\n\
infamous notorious\n\
infeasibility unfeasibility\n\
infected septic\n\
infectious infective\n\
infective morbific pathogenic\n\
infelicitous unhappy\n\
inferior subscript\n\
infertile sterile unfertile\n\
infest overrun\n\
infidelity unfaithfulness\n\
infinite nonfinite\n\
infinite space\n\
infinitesimal minute\n\
inflater inflator\n\
inflation ostentation ostentatiousness pomposity pompousness pretentiousness puffiness splashiness\n\
inflexibility inflexibleness\n\
inflexibility rigidity rigidness\n\
inflexible rigid unbending\n\
inflexible sturdy uncompromising\n\
infliximab remicade\n\
inflowing influent\n\
informant source\n\
informant witness witnesser\n\
informative informatory\n\
informative instructive\n\
inframaxillary mandibular\n\
infrastructure substructure\n\
infrequency rareness rarity\n\
inger ingerman ingrian\n\
ingratiating ingratiatory insinuating\n\
ingrowing ingrown\n\
inh isoniazid nydrazid\n\
inhalant inhalation\n\
inhalator inhaler\n\
inhalator respirator\n\
inharmonious unharmonious\n\
inherent underlying\n\
inhibitory repressing repressive\n\
inhomogeneous nonuniform\n\
inhumaneness inhumanity\n\
inimical unfriendly\n\
iniquitous sinful ungodly\n\
initiate pundit savant\n\
initiation knowledgeability knowledgeableness\n\
initiator instigator\n\
inject shoot\n\
injectant injection\n\
injun redskin\n\
injured offended pained\n\
injustice unjustness\n\
inkstand inkwell\n\
inlet intake\n\
inlet recess\n\
inmate inpatient\n\
inmost innermost\n\
innate unconditioned unlearned\n\
inner inside privileged\n\
inner interior internal\n\
inner internal\n\
innersole insole\n\
innocuous unobjectionable\n\
innovation invention\n\
inoculant inoculum\n\
inoculator vaccinator\n\
inodorous odorless odourless\n\
inoffensive unoffending\n\
inopportuneness untimeliness\n\
inquisitive questioning speculative wondering\n\
inquisitor interrogator\n\
inquisitory probing searching\n\
insalubrious unhealthful unhealthy\n\
insalubriousness insalubrity\n\
insanitary unhealthful unsanitary\n\
insatiable insatiate unsatiable\n\
insect louse worm\n\
insecure unsafe\n\
inseminate sow\n\
insensate insentient\n\
insensible senseless\n\
insensible unaffected\n\
insensitiveness insensitivity\n\
insert inset\n\
insert tuck\n\
insessores percher\n\
inshore onshore shoreward\n\
inside interior\n\
insidious pernicious subtle\n\
insignificant peanut\n\
insignificant unimportant\n\
insipid jejune\n\
insistent repetitive\n\
insolubility unsolvability\n\
insolvable unresolvable unsoluble unsolvable\n\
insomniac sleepless watchful\n\
insouciant nonchalant\n\
inspect visit\n\
inst instant\n\
instability unstableness\n\
instal install\n\
instant instantaneous\n\
instil instill\n\
instinct replete\n\
institutionalised institutionalized\n\
instructor teacher\n\
instrument pawn\n\
instrumentalist musician player\n\
instrumentality instrumentation\n\
insubordinate resistant resistive\n\
insubstantial unreal unsubstantial\n\
insufferable unsufferable\n\
insulant insulation\n\
insular parochial\n\
insuperable insurmountable\n\
insuperable unconquerable\n\
insurgent insurrectionist rebel\n\
insurgent seditious subversive\n\
insurmountable unsurmountable\n\
insurrectional insurrectionary\n\
insusceptible unsusceptible\n\
intact inviolate\n\
intangible nonphysical\n\
integrated structured\n\
integrator planimeter\n\
integumental integumentary\n\
intellect intellectual\n\
intellectual noetic rational\n\
intelligent reasoning thinking\n\
intense vivid\n\
intensity intensiveness\n\
intensity loudness volume\n\
interactional interactive\n\
interactive synergistic\n\
intercessor intermediary intermediator mediator\n\
interchurch interdenominational\n\
interconnect interlink\n\
interconnected interrelated\n\
interdependent mutualist\n\
interest interestingness\n\
interest sake\n\
interface port\n\
interior internal national\n\
interior midland upcountry\n\
interlace interlock lock\n\
interlacing interlinking interlocking interwoven\n\
interlineal interlinear\n\
interlock lock\n\
interlocutor middleman\n\
interloper intruder trespasser\n\
intermediate medium\n\
intermittence intermittency\n\
internal intragroup\n\
internality inwardness\n\
internationalism internationality\n\
internationalist internationalistic\n\
interpenetrate permeate\n\
interpretative interpretive\n\
interpreted taken\n\
interpreter representative spokesperson voice\n\
interpreter translator\n\
interracial mixed\n\
interrogative interrogatory\n\
interscholastic interschool\n\
interspecies interspecific\n\
interval separation\n\
interweave weave\n\
intolerable unbearable unendurable\n\
intoxicant intoxicating\n\
intractability intractableness\n\
intracutaneous intradermal intradermic\n\
intransigence intransigency\n\
intraspecies intraspecific\n\
intrinsic intrinsical\n\
introductory prefatorial prefatory\n\
introspective introverted\n\
introversive introvertive\n\
introvert invaginate\n\
intrude irrupt\n\
intrusiveness meddlesomeness officiousness\n\
intuitive nonrational visceral\n\
intumescent puffy tumescent tumid turgid\n\
inutility unusefulness uselessness\n\
invalidated nullified\n\
invalidator nullifier voider\n\
invalidity invalidness\n\
invaluable priceless\n\
invaluableness preciousness pricelessness valuableness\n\
invariability invariableness invariance\n\
inverse reverse\n\
invertase saccharase sucrase\n\
invertebrate spineless\n\
investigative investigatory\n\
investigator researcher\n\
invigorated refreshed reinvigorated\n\
invincible unbeatable unvanquishable\n\
inviolable inviolate sacrosanct\n\
invirase saquinavir\n\
invisibility invisibleness\n\
invisible unseeable\n\
invite receive\n\
involuntariness unwillingness\n\
involuntary nonvoluntary unvoluntary\n\
involute rolled\n\
involved mired\n\
iodin iodine\n\
iodinated iodised iodized\n\
iodoform triiodomethane\n\
ionised ionized\n\
iota scintilla shred smidge smidgen smidgeon smidgin tittle whit\n\
iowa ioway\n\
ipidae scolytidae\n\
ir iridium\n\
irak iraq\n\
iraki iraqi\n\
iran persia\n\
irani iranian persian\n\
iranian persian\n\
irate ireful\n\
iridescence opalescence\n\
iridescent nacreous opalescent opaline pearlescent\n\
iridosmine osmiridium\n\
iron press\n\
ironic ironical\n\
ironist ridiculer satirist\n\
ironweed vernonia\n\
irreclaimable irredeemable unredeemable unreformable\n\
irreconcilable unreconcilable\n\
irrecoverable unrecoverable\n\
irredenta irridenta\n\
irredentist irridentist\n\
irregularity unregularity\n\
irreligion irreligiousness\n\
irreplaceable unreplaceable\n\
irrepressible uncontrollable\n\
irreproducible unreproducible\n\
irresistibility irresistibleness\n\
irresistible resistless\n\
irresoluteness irresolution\n\
irresponsibility irresponsibleness\n\
irretrievable unretrievable\n\
irrevocable irrevokable\n\
irritating irritative\n\
irritating painful\n\
irruptive plutonic\n\
irtish irtysh\n\
isarithm isogram isopleth\n\
ischaemic ischemic\n\
iseult isolde\n\
ishtar mylitta\n\
isinglass mica\n\
islamic moslem muslim\n\
isle islet\n\
ismaili ismailian\n\
isocarboxazid marplan\n\
isochronal isochronous\n\
isoclinal isoclinic\n\
isolationist isolationistic\n\
isometric isometrical\n\
isomorphic isomorphous\n\
isomorphism isomorphy\n\
isoproterenol isuprel\n\
isordil isosorbide\n\
isosmotic isotonic\n\
isotropic isotropous\n\
isotropy symmetry\n\
israel sion yisrael zion\n\
issue offspring progeny\n\
italia italy\n\
iterative reiterative\n\
ithaca ithaki\n\
itinerary path route\n\
itraconazole sporanox\n\
ivory tusk\n\
ix nine\n\
izmir smyrna\n\
jab stab\n\
jabalpur jubbulpore\n\
jabiru saddlebill\n\
jackanapes lightweight whippersnapper\n\
jackfruit jak\n\
jackstraw spillikin\n\
jacobinic jacobinical\n\
jactation jactitation\n\
jade jadestone\n\
jaded wearied\n\
jafar jaffar\n\
jaffa joppa yafo\n\
jagannath jagannatha jagganath juggernaut\n\
jaggary jaggery jagghery\n\
jagged jaggy scraggy\n\
jaguar panther\n\
jahvey jahweh jehovah jhvh wahvey yahve yahveh yahwe yahweh yhvh yhwh\n\
jain jainist\n\
jakes outhouse privy\n\
jam mob throng\n\
jamberry miltomate tomatillo\n\
jamberry tomatillo\n\
jamjar jampot\n\
jammed packed\n\
jammies pajama pyjama\n\
jangling jangly\n\
japan nihon nippon\n\
japanese nipponese\n\
jar jarful\n\
jar jolt\n\
jargon jargoon\n\
jaunt travel trip\n\
javan javanese\n\
jawbone jowl mandible mandibula submaxilla\n\
jealous overjealous\n\
jeddah jidda jiddah\n\
jeep landrover\n\
jejuneness jejunity\n\
jejuneness jejunity tameness vapidity vapidness\n\
jemmy jimmy\n\
jennet jenny\n\
jerker yanker\n\
jerkwater pokey poky\n\
jersey nj\n\
jesting jocose jocular joking\n\
jesuit jesuitic jesuitical\n\
jet pitchy sooty\n\
jetting spouting spurting squirting\n\
jeweler jeweller\n\
jewellery jewelry\n\
jewfish mulloway\n\
jewish judaic\n\
jfk kennedy\n\
jigger jiggermast\n\
jigger pony\n\
jiggle joggle wiggle\n\
jilted rejected spurned\n\
jimmies sprinkles\n\
jimmy lever prise prize pry\n\
jingling jingly\n\
jinrikisha ricksha rickshaw\n\
jinx jonah\n\
jnr jr junior\n\
jobber middleman wholesaler\n\
jock jockstrap suspensor\n\
jocote mombin\n\
jocund jolly jovial merry mirthful\n\
john trick whoremaster whoremonger\n\
johnson lbj\n\
johor johore\n\
joined united\n\
joint reefer spliff\n\
joint roast\n\
jointworm strawworm\n\
joker jokester\n\
joker turkey\n\
joliet jolliet\n\
jolted shaken\n\
jongleur minstrel troubadour\n\
jook juke\n\
jostle shove\n\
jotun jotunn\n\
journey travel\n\
journeyer wayfarer\n\
jove jupiter\n\
joyride tool\n\
juda judah\n\
judaea judea\n\
judaic judaical\n\
judas jude thaddaeus\n\
judge jurist justice\n\
judgement judgment perspicacity\n\
judicial juridic juridical\n\
judiciousness sagaciousness sagacity\n\
jug jugful\n\
juggernaut steamroller\n\
jugoslav jugoslavian yugoslav yugoslavian\n\
jugoslavija yugoslavia\n\
juice succus\n\
juicer reamer\n\
juiciness succulence succulency\n\
juicy luscious toothsome voluptuous\n\
jukebox nickelodeon\n\
jumbal jumble\n\
jumble scramble\n\
jumper pinafore pinny\n\
jumper sweater\n\
juncaginaceae scheuchzeriaceae\n\
junco snowbird\n\
juneberry saskatoon serviceberry shadberry\n\
juneberry serviceberry shadblow shadbush\n\
juniper raetam retem\n\
junket junketeer\n\
junkie junky\n\
junoesque statuesque\n\
jupati jupaty\n\
jural juristic\n\
juridic juridical\n\
juror juryman jurywoman\n\
just upright\n\
justice justness\n\
justiciar justiciary\n\
justificative justificatory vindicatory\n\
justness nicety rightness\n\
jutland jylland\n\
jutting projected projecting protruding relieved sticking\n\
juvenility youth youthfulness\n\
kabob kebab\n\
kaleidoscopic kaleidoscopical\n\
kalka khalka khalkha\n\
kanamycin kantrex\n\
kananga luluabourg\n\
kanchanjanga kanchenjunga kinchinjunga\n\
kandahar qandahar\n\
kandinski kandinsky\n\
kansa kansas\n\
kansas ks\n\
kaochlor klorvess\n\
kaolin kaoline\n\
karakoram mustagh\n\
karbala kerbala kerbela\n\
karnataka mysore\n\
kartikeya karttikeya\n\
karyon nucleus\n\
karyoplasm nucleoplasm\n\
katar qatar\n\
katari qatari\n\
kathmandu katmandu\n\
kaunas kovna kovno\n\
kauri kaury\n\
kava kavakava\n\
kavrin papaverine\n\
kayoed out stunned\n\
kazak kazakh\n\
kazak kazakh kazakhstan kazakstan\n\
kb kbit kilobit\n\
kb kib kibibyte kilobyte\n\
kb kilobyte\n\
kechua quechua\n\
kechuan quechuan\n\
keepsake relic souvenir token\n\
keg kegful\n\
kelpie kelpy\n\
kemadrin procyclidine\n\
kempt tidy\n\
kennedia kennedya\n\
kentucky ky\n\
kept unbroken\n\
kernel meat\n\
kerosene kerosine\n\
ketalar ketamine\n\
ketembilla kitambilla kitembilla\n\
ketoprofen orudis oruvail\n\
ketorolac torodal\n\
kettle kettledrum timpani tympani tympanum\n\
kettle kettleful\n\
kg kilo kilogram\n\
khaddar khadi\n\
khanty ostyak\n\
kharkiv kharkov\n\
khirghiz kirghiz kirgiz\n\
kibibit kibit\n\
kick recoil\n\
kid kidskin\n\
kid kyd\n\
kiddie kiddy\n\
kiev kyyiv\n\
kildeer killdeer\n\
kiley kylie\n\
kiliwa kiliwi\n\
killer slayer\n\
killing sidesplitting\n\
killjoy spoilsport\n\
kiloliter kilolitre\n\
kilometer kilometre klick km\n\
kilovolt kv\n\
kilowatt kw\n\
kinaesthetic kinesthetic\n\
kind tolerant\n\
kindergartener kindergartner preschooler\n\
kindling punk spunk tinder touchwood\n\
king queen\n\
king rex\n\
kingbolt kingpin\n\
kingdom realm\n\
kinglike kingly\n\
kingmaker warwick\n\
kink twirl\n\
kinkajou potto\n\
kinky offbeat quirky\n\
kinky perverted\n\
kinshasa leopoldville\n\
kirghiz kirghizia kirghizstan kirgiz kirgizia kirgizstan kyrgyzstan\n\
kisser osculator\n\
kit outfit\n\
kitten kitty\n\
kittul kitul\n\
kitty puss pussy pussycat\n\
klaipeda memel\n\
klansman kluxer\n\
knackwurst knockwurst\n\
knap rap\n\
knave rapscallion rascal rogue scalawag scallywag varlet\n\
knawe knawel\n\
knead massage\n\
knee stifle\n\
kneecap kneepan patella\n\
knickknack knickknackery nicknack whatnot\n\
knickknack novelty\n\
knife stab\n\
knife tongue\n\
knit knitting knitwork\n\
knob node thickening\n\
knob pommel\n\
knobbly knobby\n\
knobkerrie knobkerry\n\
knot ravel tangle\n\
knotty snarled snarly\n\
knowing knowledgeable\n\
knowing knowledgeable learned lettered\n\
knowing wise\n\
knowledgeable versed\n\
knuckles knucks\n\
koellia pycnanthemum\n\
konoe konoye\n\
koodoo koudou kudu\n\
kopje koppie\n\
kosciusko kosciuszko\n\
kota kotar\n\
koumiss kumis\n\
koweit kuwait\n\
kr krypton\n\
krakatao krakatau krakatoa\n\
kuenlun kunlun\n\
kulun ulaanbaatar urga\n\
kurus piaster piastre\n\
kwai yuan\n\
la lanthanum\n\
la louisiana\n\
laager lager\n\
lab laboratory\n\
labdanum ladanum\n\
label tag\n\
labeled labelled tagged\n\
labetalol normodyne trandate\n\
labiatae lamiaceae\n\
labiate liplike\n\
labored laboured\n\
labored laboured strained\n\
laborer labourer\n\
laboriousness operoseness toilsomeness\n\
laborsaving laboursaving\n\
labrocyte mastocyte\n\
labyrinth maze\n\
labyrinthian labyrinthine mazy\n\
labyrinthodonta labyrinthodontia\n\
lace lacing\n\
laced tied\n\
lacelike lacy\n\
lacerate lacerated\n\
lacerate lacerated mangled torn\n\
lacertilia sauria\n\
lacertilian saurian\n\
lacewood sycamore\n\
lachrymal lacrimal\n\
lachrymator lacrimator teargas\n\
lachrymatory lacrimatory\n\
lackluster lacklustre lusterless lustreless\n\
lactaid lactase\n\
lactating wet\n\
lactobacillaceae lactobacteriaceae\n\
lacy netlike netted webbed webby weblike\n\
lade laden\n\
lade laden ladle\n\
laden ladened loaded\n\
laden oppressed\n\
lady noblewoman peeress\n\
ladybeetle ladybird ladybug\n\
ladyfish tenpounder\n\
laffite lafitte\n\
lag stave\n\
lagan lagend ligan\n\
lagoon laguna lagune\n\
laic lay secular\n\
lakeshore lakeside\n\
lakota teton\n\
lambskin parchment sheepskin\n\
lamellibranch pelecypod pelecypodous\n\
lamenting wailful wailing\n\
lamia vampire\n\
laminal laminar\n\
lamisil terbinafine\n\
lammergeier lammergeyer\n\
lampooner parodist\n\
lampshade shade\n\
lanate woolly\n\
lance lancet\n\
lance spear\n\
lancelike lanceolate\n\
lancetfish wolffish\n\
lanchou lanchow lanzhou\n\
landholder landowner\n\
landlubber landman landsman\n\
landlubber landsman lubber\n\
landlubberly lubberly\n\
landscaper landscapist\n\
langobard lombard\n\
langoustine scampo\n\
langsat langset\n\
laniard lanyard\n\
lank spindly\n\
lansa lansat lanseh lanset\n\
lansoprazole prevacid\n\
lanthanide lanthanoid lanthanon\n\
lao laotian\n\
lap lick\n\
lap overlap\n\
lapidarist lapidary\n\
lapidary lapidist\n\
lapidate stone\n\
lapidator stoner\n\
lapin rabbit\n\
lapland lappland\n\
lapp lapplander saame saami same sami\n\
lappet wattle\n\
lapsed nonchurchgoing\n\
lapwing peewit pewit\n\
larboard port\n\
larcener larcenist\n\
larcenous thievishness\n\
largeness pretension pretentiousness\n\
largess largesse magnanimity munificence openhandedness\n\
lariat lasso reata riata\n\
larium mefloquine mephaquine\n\
lark meadowlark\n\
lark pipit titlark\n\
larrup paddle spank\n\
lasagna lasagne\n\
lascivious lewd libidinous lustful\n\
lash thong\n\
lash whip\n\
lass lassie\n\
lassa lhasa\n\
lasso rope\n\
lasting permanent\n\
lasting persistent\n\
late later\n\
late recent\n\
later posterior ulterior\n\
lateral sidelong\n\
latest modish\n\
lathee lathi\n\
lather soapsuds suds\n\
lathery sudsy\n\
latin romance\n\
latitude parallel\n\
latitudinarian undogmatic undogmatical\n\
latium lazio\n\
latona leto\n\
laudability laudableness praiseworthiness\n\
laudatory praiseful praising\n\
laughing riant\n\
launder wash\n\
launderette laundromat\n\
laundress laundrywoman washerwoman washwoman\n\
laundry wash washables washing\n\
laundryman washerman\n\
laurentius lawrence\n\
lavalier lavaliere lavalliere\n\
lavender lilac\n\
lavish lucullan plush plushy\n\
lavish munificent overgenerous unsparing unstinted unstinting\n\
lavishness luxury sumptuosity sumptuousness\n\
lawbreaker violator\n\
lawful legitimate licit\n\
lawful rightful\n\
lawgiver lawmaker\n\
lawless outlaw\n\
lawlessness outlawry\n\
lawrencium lr\n\
lax slack\n\
laxity laxness remissness slackness\n\
lay pose position put\n\
lay repose\n\
layered superimposed\n\
layman layperson secular\n\
lazar leper\n\
lazaret lazarette lazaretto pesthouse\n\
lea ley pasture pastureland\n\
leach percolate\n\
leadbelly ledbetter\n\
leaden plodding\n\
leaden weighted\n\
leading preeminent\n\
leading prima star starring stellar\n\
leadless unleaded\n\
leafed leaved\n\
leafstalk petiole\n\
leanness spareness thinness\n\
leap spring\n\
leaseholder lessee\n\
leash rope\n\
leash tether\n\
leatherfish leatherjacket\n\
leatherjack leatherjacket\n\
leatherneck marine\n\
leatherwood moosewood ropebark wicopy\n\
leaven leavening\n\
leaven prove\n\
lech lecher letch satyr\n\
lector lecturer\n\
ledge shelf\n\
lee leeward\n\
leech parasite sponge sponger\n\
leechee lichee lichi litchee litchi lychee\n\
leek scallion\n\
leery mistrustful suspicious untrusting wary\n\
leeuwenhoek leuwenhoek\n\
left leftfield\n\
left leftover odd remaining unexpended\n\
lefthander lefty southpaw\n\
lefty southpaw\n\
leg peg pegleg\n\
legging leging\n\
legibility readability\n\
legion numerous\n\
legionary legionnaire\n\
legitimate logical\n\
leibnitz leibniz\n\
leibnitzian leibnizian\n\
leicester leicestershire\n\
leiden leyden\n\
leiopelma liopelma\n\
leiopelmatidae liopelmidae\n\
leipoa lowan\n\
lemanderin rangpur\n\
lemnos limnos\n\
lemon stinker\n\
lemonlike lemony sourish tangy tart\n\
lender loaner\n\
lengthways lengthwise\n\
lenience leniency lenity mildness\n\
leningrad peterburg petrograd\n\
lens lense\n\
lensman photographer\n\
lentia linz\n\
lentisk mastic\n\
leo lion\n\
leoncita tamarin\n\
leotard unitard\n\
leotards tights\n\
lepechinia sphacele\n\
lepidopteran lepidopteron\n\
lepidopterist lepidopterologist\n\
lepidote leprose scabrous scaly scurfy\n\
leptorhine leptorrhine leptorrhinian leptorrhinic\n\
ler lir\n\
lesbian sapphic\n\
lesbian tribade\n\
lesbos lesvos mytilene\n\
lethargic unenergetic\n\
leucocyte leukocyte wbc\n\
leucocytozoan leucocytozoon\n\
leucorrhea leukorrhea\n\
levallorphan lorfan\n\
leveler leveller\n\
leverage purchase\n\
levi matthew\n\
levitra vardenafil\n\
levorotary levorotatory\n\
lewd obscene raunchy salacious\n\
lexicalised lexicalized\n\
lexicographer lexicologist\n\
lexicographic lexicographical\n\
li lithium\n\
liable nonimmune nonresistant unresistant\n\
liakoura parnassus\n\
liar prevaricator\n\
liberal liberalist progressive\n\
liberal tolerant\n\
liberality liberalness\n\
licentiousness wantonness\n\
lichee litchi\n\
lichgate lychgate\n\
licorice liquorice\n\
lidless sleepless\n\
lidocaine xylocaine\n\
liege luik\n\
lien spleen\n\
lienal splenetic splenic\n\
lietuva lithuania\n\
life liveliness spirit sprightliness\n\
lifeguard lifesaver\n\
lifted upraised\n\
lifter weightlifter\n\
lighted lit\n\
lightheaded swooning\n\
lightless unilluminated unlighted unlit\n\
lightness lightsomeness\n\
lightness weightlessness\n\
lightsome tripping\n\
likable likeable\n\
like same\n\
like similar\n\
likelihood likeliness\n\
likely potential\n\
likely probable\n\
likeness semblance\n\
liliopsida monocotyledonae monocotyledones\n\
lilt swing\n\
lilting swinging swingy tripping\n\
limacine limacoid\n\
limber supple\n\
limit limitation\n\
limited modified\n\
limitless unlimited\n\
limner portraitist portrayer\n\
limo limousine\n\
limp wilted\n\
limpid lucid luculent pellucid perspicuous\n\
limpidity pellucidity pellucidness\n\
linage lineage\n\
linchpin lynchpin\n\
lincocin lincomycin\n\
lincolnesque lincolnian\n\
lindesnes naze\n\
line pipeline\n\
linear running\n\
lineation outline\n\
lined seamed\n\
liner lining\n\
linger tarry\n\
lingerer loiterer\n\
lingual linguistic\n\
linguine linguini\n\
linguist polyglot\n\
link linkup\n\
link yoke\n\
linkboy linkman\n\
linnaean linnean\n\
linnet lintwhite\n\
lino linoleum\n\
liothyronine triiodothyronine\n\
liparidae liparididae\n\
lipid lipide lipoid\n\
lipizzan lippizan lippizaner\n\
lipless unlipped\n\
lipophilic lipotropic\n\
liquefiable liquifiable\n\
liquefied liquified\n\
liquefied liquified molten\n\
liquescent melting\n\
liquidate neutralise neutralize waste\n\
liquidator manslayer murderer\n\
liquidator receiver\n\
liquified melted\n\
lisboa lisbon\n\
lisinopril prinival zestril\n\
lissom lissome lithe lithesome sinuous supple\n\
lissomeness litheness suppleness\n\
lister middlebreaker\n\
listlessness torpidity torpidness torpor\n\
liter litre\n\
lithops stoneface\n\
litigant litigator\n\
litoral littoral sands\n\
litterbug litterer\n\
littleness pettiness smallness\n\
littleness smallness\n\
littler smaller\n\
livable liveable\n\
live unrecorded\n\
livelong orpin orpine\n\
lively racy\n\
lively vital\n\
liverpudlian scouser\n\
living surviving\n\
lizardfish snakefish\n\
lm lumen\n\
loadstar lodestar\n\
loadstone lodestone\n\
loath loth reluctant\n\
loathsome nauseating nauseous noisome offensive queasy sickening vile\n\
loathsomeness lousiness repulsiveness sliminess vileness wickedness\n\
lobate lobated\n\
lobate lobed\n\
lobscouse lobscuse scouse\n\
lobsterback redcoat\n\
lobworm lugworm\n\
locale locus venue\n\
localised localized\n\
locality neighborhood neighbourhood vicinity\n\
located placed situated\n\
locater locator\n\
lockkeeper lockman lockmaster\n\
locomote move travel\n\
locomotion motivity\n\
locomotive locomotor\n\
locule loculus\n\
lodgement lodging lodgment\n\
loftiness majesty stateliness\n\
lofty majestic proud\n\
log lumber\n\
logicality logicalness\n\
logician logistician\n\
logistic logistical\n\
logogrammatic logographic\n\
logomach logomachist\n\
logos son word\n\
loin lumbus\n\
loins pubes\n\
lollipop lolly popsicle\n\
lollipop sucker\n\
lombardia lombardy\n\
lone lonely\n\
lone lonesome only sole\n\
lonely lonesome\n\
lonely unfrequented\n\
long recollective retentive tenacious\n\
longan longanberry lungen\n\
longer thirster yearner\n\
longevity seniority\n\
longlegs stilt stiltbird\n\
longyi lungi lungyi\n\
loniten minoxidil rogaine\n\
lontar palmyra\n\
loofa loofah luffa\n\
looker spectator viewer watcher witness\n\
looking sounding\n\
lookout observatory\n\
lookout picket scout sentinel sentry spotter watch\n\
loosen tease\n\
loosen undo untie\n\
looseness play\n\
looted pillaged plundered ransacked\n\
lopper pruner\n\
lopressor metoprolol\n\
lopsidedness skewness\n\
lord noble nobleman\n\
lord overlord\n\
lordless masterless\n\
lorraine lothringen\n\
loudspeaker speaker\n\
louisianan louisianian\n\
lounger recliner\n\
lovable loveable\n\
lovastatin mevacor\n\
loverlike loverly\n\
lowbrow lowbrowed uncultivated\n\
lowbrow philistine\n\
lowerclassman underclassman\n\
lowering sullen threatening\n\
lowly petty secondary subaltern\n\
loxapine loxitane\n\
loxodrome rhumb\n\
loyal patriotic\n\
loyalist stalwart\n\
loyalty trueness\n\
loyang luoyang\n\
lozenge pill tab tablet\n\
lu lutecium lutetium\n\
lube lubricant lubricator\n\
lube lubricate\n\
lubricious lustful prurient salacious\n\
lucifugal lucifugous\n\
lucite perspex\n\
luckless unlucky\n\
lucrative moneymaking remunerative\n\
luda luta\n\
luge toboggan\n\
luger slider\n\
lukewarm tepid\n\
lukewarmness tepidity tepidness\n\
lukewarmness tepidness\n\
lulli lully\n\
lumber timber\n\
lumbering ponderous\n\
lumbermill sawmill\n\
luminal phenobarbital phenobarbitone\n\
luminary notability notable\n\
lumpen lumpish unthinking\n\
lunatic madman maniac\n\
lunatic moonstruck\n\
lunula lunule\n\
lupin lupine\n\
lurcher lurker skulker\n\
lurid shocking\n\
lushness luxuriance voluptuousness\n\
lusitanian portuguese\n\
luster lustre\n\
luster lustre sheen shininess\n\
lutanist lutenist lutist\n\
lute luting\n\
lutefisk lutfisk\n\
lutein xanthophyl xanthophyll\n\
luteotropin prolactin\n\
lux lx\n\
luxembourg luxemburg\n\
luxembourger luxemburger\n\
lycanthrope werewolf wolfman\n\
lycopersicon lycopersicum\n\
lycopodiate lycopsida\n\
lyon lyons\n\
lyophilised lyophilized\n\
lyric lyrical\n\
lyricality lyricism songfulness\n\
lyricist lyrist\n\
lysichiton lysichitum\n\
lysozyme muramidase\n\
ma mama mamma mammy mom momma mommy mum mummy\n\
ma massachusetts\n\
ma milliampere\n\
maarianhamina mariehamn\n\
mac macintosh mack mackintosh\n\
macadam tarmac tarmacadam\n\
macadamise macadamize tarmac\n\
macao macau\n\
mace macebearer macer\n\
macedon macedonia makedonija\n\
machete matchet panga\n\
machinist mechanic\n\
macintosh mackintosh\n\
macon maconnais\n\
macrencephalic macrencephalous\n\
macrocephalic macrocephalous\n\
macrocyte megalocyte\n\
macrodantin nitrofurantoin\n\
macromolecule supermolecule\n\
macroscopic macroscopical\n\
macrosporangium megasporangium\n\
macrospore megaspore\n\
macrouridae macruridae\n\
macula macule\n\
macula sunspot\n\
madness rabidity rabidness\n\
madonna mary\n\
madrona madrono manzanita\n\
maghreb mahgrib\n\
magician necromancer sorcerer thaumaturge thaumaturgist wizard\n\
magilp megilp\n\
magnesia periclase\n\
magnesium mg\n\
magnetic magnetised magnetized\n\
magnetisation magnetization\n\
magpie scavenger\n\
maha omaha\n\
mahagua mahoe majagua\n\
maharaja maharajah\n\
maharanee maharani\n\
mahican mohican\n\
mahlstick maulstick\n\
mahogany sepia\n\
mahomet mahound mohammad mohammed muhammad\n\
mahratta maratha\n\
maia maja\n\
maid maiden\n\
maidenlike maidenly\n\
maiger maigre\n\
mailbag postbag\n\
mailboat packet\n\
mailbox postbox\n\
maimed mutilated\n\
maimer mangler mutilator\n\
maine me\n\
mainsheet sheet shroud tack\n\
mainstay pillar\n\
maintained retained\n\
maintainer sustainer upholder\n\
maiolica majolica\n\
maisonette maisonnette\n\
majestic olympian\n\
maker shaper\n\
making qualification\n\
makomako wineberry\n\
malachi malachias\n\
malamute malemute\n\
malanga spoonflower tannia yautia\n\
malawi nyasaland\n\
malay malayan\n\
malayan malaysian\n\
maldivan maldivian\n\
male manful manlike manly virile\n\
maleness masculinity\n\
malevolence malevolency malice\n\
malfunction misfunction\n\
malfunctioning nonfunctional\n\
malignance malignancy malignity\n\
malingerer shammer skulker\n\
mall plaza\n\
mall promenade\n\
malleability plasticity\n\
malodorous malodourous stinky\n\
malposition misplacement\n\
malt malted\n\
maltman maltster\n\
mamey mammee\n\
mamilla mammilla nipple pap teat tit\n\
mammal mammalian\n\
mammee sapote\n\
manageability manageableness\n\
manakin manikin mannequin mannikin\n\
manakin manikin mannequin mannikin model\n\
manawydan manawyddan\n\
mandaean mandean\n\
mandatary mandatory\n\
mandate mandatory\n\
mandelamine methenamine urex\n\
mandelshtam mandelstam\n\
maneuverability manoeuvrability\n\
maneuverable manoeuvrable\n\
maneuverer manoeuvrer\n\
manful manlike manly\n\
manfulness manliness virility\n\
manganese mn\n\
manger trough\n\
mangey mangy\n\
manginess seediness shabbiness sleaziness\n\
mangle maul\n\
maniac maniacal\n\
manichaean manichean manichee\n\
manifold multiplex\n\
manila manilla\n\
manipulable tractable\n\
manipulator operator\n\
mannitol osmitrol\n\
manroot scammonyroot\n\
mansi vogul\n\
manteidae mantidae\n\
mantelet mantilla\n\
mantelet mantlet\n\
mantichora manticora manticore mantiger\n\
mantid mantis\n\
manufacturer producer\n\
manure muck\n\
mapper plotter\n\
maquis maquisard\n\
marabou marabout\n\
marauder piranha predator vulture\n\
marauding predatory raiding\n\
marbled marbleised marbleized\n\
marbleisation marbleising marbleization marbleizing\n\
march process\n\
marche marches\n\
marcher parader\n\
marchioness marquise\n\
marchpane marzipan\n\
marduk merodach\n\
mare maria\n\
margarin margarine marge oleo oleomargarine\n\
margin perimeter\n\
marginocephalia marginocephalian\n\
marimba xylophone\n\
marine maritime nautical\n\
marini marino\n\
marionette puppet\n\
marital married matrimonial\n\
marjoram oregano\n\
marked pronounced\n\
market marketplace mart\n\
marketable merchantable sellable vendable vendible\n\
marketer seller trafficker vender vendor\n\
markhoor markhor\n\
markoff markov\n\
marksman sharpshooter\n\
marlinespike marlingspike marlinspike\n\
marlite marlstone\n\
marmara marmora\n\
marmoreal marmorean\n\
maroc marruecos morocco\n\
maroc moroccan\n\
marooned stranded\n\
marquee marquise\n\
marquee pavilion\n\
marquess marquis\n\
marqueterie marquetry\n\
marrakech marrakesh\n\
marred scarred\n\
marriageable nubile\n\
marseille marseilles\n\
marshal marshall\n\
martial soldierlike soldierly warriorlike\n\
martial warlike\n\
martyr sufferer\n\
marveller wonderer\n\
marvellous marvelous miraculous\n\
marx zeppo\n\
maryland md\n\
masher wolf\n\
mashhad meshed\n\
masjid musjid\n\
masker masquer masquerader\n\
mason stonemason\n\
masorete masorite massorete\n\
masqat muscat\n\
massachuset massachusetts\n\
massicot massicotite\n\
massive monolithic monumental\n\
mastaba mastabah\n\
mastigomycota mastigomycotina\n\
mastodon mastodont\n\
mastoid mastoidal\n\
masturbate wank\n\
masturbator onanist\n\
matchbush matchweed\n\
matcher matchmaker\n\
matchless nonpareil peerless unmatchable unmatched unrivaled unrivalled\n\
matchwood splinters\n\
mated paired\n\
materfamilias matriarch\n\
material stuff\n\
materialistic mercenary\n\
maternal parental paternal\n\
maternalism maternity motherliness\n\
mathematical numerical\n\
matoaka pocahontas\n\
matrilineal matrilinear\n\
matt matte matted\n\
matteuccia pteretis\n\
mature matured\n\
mature ripe\n\
matzah matzo matzoh\n\
maul sledge sledgehammer\n\
mauritania mauritanie muritaniya\n\
mauritanian mauritian\n\
maverick rebel\n\
maverick unorthodox\n\
mavik trandolapril\n\
mavis throstle\n\
mawlamyine moulmein\n\
maxilla maxillary\n\
maximal maximum\n\
maximising maximizing\n\
maximum utmost uttermost\n\
maxwell mx\n\
may whitethorn\n\
maya mayan\n\
mayo mayonnaise\n\
mb mbit megabit\n\
mb mebibyte megabyte mib\n\
mb megabyte\n\
mcg microgram\n\
md mendelevium mv\n\
mdma methylenedioxymethamphetamine\n\
meager meagerly meagre scrimpy stingy\n\
meal repast\n\
meander thread wander weave wind\n\
meandering rambling wandering winding\n\
meanie meany\n\
meaning pregnant significant\n\
meaningless nonmeaningful\n\
meanness minginess niggardliness niggardness parsimoniousness parsimony tightfistedness tightness\n\
meanspirited ungenerous\n\
measly paltry\n\
measurability quantifiability\n\
measurable mensurable\n\
measured mensurable mensural\n\
measured metric metrical\n\
meaty substantive\n\
mebaral mephobarbital\n\
mebibit mibit\n\
mechanical technical\n\
mechanised mechanized\n\
mechanised mechanized motorized\n\
mecholyl methacholine\n\
meclofenamate meclomen\n\
medalist medallist\n\
mediacy mediateness\n\
mediaeval medieval\n\
medial median\n\
mediate middle\n\
medic medick trefoil\n\
medicament medication medicine\n\
medicative medicinal\n\
mediety moiety\n\
mediocre middling\n\
medium sensitive spiritualist\n\
medroxyprogesterone provera\n\
medulla myelin myeline\n\
medullated myelinated\n\
medusa medusan medusoid\n\
meek mild modest\n\
meek spiritless\n\
meek tame\n\
meekness subduedness\n\
meerkat mierkat\n\
meerschaum sepiolite\n\
megaflop mflop\n\
megalomaniacal megalomanic\n\
megalosaur megalosaurus\n\
megatherian megatheriid\n\
meitnerium mt\n\
melaena melena\n\
melancholiac melancholic\n\
melancholic melancholy\n\
melancholy somber sombre\n\
melastomaceae melastomataceae\n\
melchite melkite\n\
melicocca melicoccus\n\
melilot melilotus\n\
mellaril thioridazine\n\
mellow mellowed\n\
melodic melodious musical\n\
melodious tuneful\n\
melodiousness tunefulness\n\
member penis phallus\n\
memorial monument\n\
memoriser memorizer\n\
memory storage store\n\
menagerie zoo\n\
mendeleev mendeleyev\n\
meninges meninx\n\
menominee menomini\n\
mensch mensh\n\
mephenytoin mesantoin\n\
mephitic miasmic\n\
meq milliequivalent\n\
merbromine mercurochrome\n\
mercantile mercenary moneymaking\n\
mercaptopurine purinethol\n\
mercerised mercerized\n\
merchandise product ware\n\
merchandiser merchant\n\
mercifulness mercy\n\
merciless unmerciful\n\
mercilessness unmercifulness\n\
mercuric mercurous\n\
meretriciousness speciousness\n\
merganser sawbill sheldrake\n\
meridian prime\n\
merit virtue\n\
meritable meritorious\n\
meritless sorry\n\
merrymaker reveler reveller\n\
merthiolate thimerosal\n\
mesa table\n\
mescal mezcal peyote\n\
mescaline peyote\n\
mesencephalon midbrain\n\
mesh meshing meshwork net network\n\
meshuga meshugga meshugge meshuggeneh meshuggener\n\
meshuggeneh meshuggener\n\
mesic mesonic\n\
mesoblast mesoderm\n\
mesoblastic mesodermal\n\
mesomorphic muscular\n\
meson mesotron\n\
mesquit mesquite\n\
messiness untidiness\n\
messy mussy\n\
metabolic metabolous\n\
metagrabolised metagrabolized metagrobolised metagrobolized mystified\n\
metal metallic\n\
metalize metallize\n\
metallurgic metallurgical\n\
metalworker smith\n\
metamere somite\n\
metameric segmental segmented\n\
metamorphic metamorphous\n\
metaphoric metaphorical\n\
metchnikoff metchnikov\n\
metempsychosis rebirth\n\
meteor meteoroid\n\
meteoric meteorologic meteorological\n\
meteoritic meteoritical\n\
meter metre\n\
meter metre time\n\
meterstick metrestick\n\
methaqualone quaalude\n\
methocarbamol robaxin\n\
methodicalness orderliness\n\
methodist wesleyan\n\
methylbenzene toluene\n\
methylphenidate ritalin\n\
meticulosity meticulousness punctiliousness scrupulousness\n\
meticulous punctilious\n\
metonymic metonymical\n\
metrazol pentamethylenetetrazol pentylenetetrazol\n\
metric metrical\n\
metro subway tube underground\n\
metycaine piperocaine\n\
mexiletine mexitil\n\
mg milligram\n\
mho siemens\n\
mi michigan\n\
mi mile\n\
miasmal miasmic vaporous vapourous\n\
mic microphone mike\n\
micah micheas\n\
michigander wolverine\n\
mick mickey paddy\n\
micmac mikmaq\n\
miconazole monistat\n\
microbial microbic\n\
microcephalic microcephalous nanocephalic\n\
microcomputer pc\n\
micrometeor micrometeorite micrometeoroid\n\
micrometeoric micrometeoritic\n\
micrometer micron\n\
micromicron picometer picometre\n\
micromillimeter micromillimetre millimicron nanometer nanometre nm\n\
micronesia tt\n\
micropenis microphallus\n\
microscopic microscopical\n\
midazolam versed\n\
middle midriff midsection\n\
middlemost midmost\n\
midline midplane\n\
midrib midvein\n\
might mightiness\n\
migrant migrator\n\
migrant migratory\n\
migrate transmigrate\n\
mikado tenno\n\
mil mile\n\
milage mileage\n\
milan milano\n\
milcher milker\n\
milfoil yarrow\n\
militarised militarized\n\
militarist warmonger\n\
milium whitehead\n\
milklike milky whitish\n\
milksop milquetoast pansy pantywaist sissy\n\
milkweed silkweed\n\
milled polished\n\
millennial millennian\n\
millepede milliped millipede\n\
millimeter millimetre mm\n\
millirem mrem\n\
millivolt mv\n\
millrace millrun\n\
milontin phensuximide\n\
mime mimer mummer pantomimer pantomimist\n\
mimeo mimeograph roneo roneograph\n\
mimic mimicker\n\
mina minah myna mynah\n\
mincing prim twee\n\
mind thinker\n\
mindless reasonless senseless\n\
mindless vacuous\n\
miner mineworker\n\
mingy miserly\n\
mini miniskirt\n\
miniature toy\n\
minibike motorbike\n\
minimal minimum\n\
minipress prazosin\n\
miniscule minuscule\n\
minisub minisubmarine\n\
minnesota mn\n\
minnewit minuit\n\
minocin minocycline\n\
minor modest\n\
minor nonaged underage\n\
minor venial\n\
minus negative\n\
minuscular minuscule\n\
minute narrow\n\
miotic myotic\n\
miraculous providential\n\
mire morass quag quagmire slack\n\
mire muck mud\n\
mire slop\n\
mirky murky\n\
mirrorlike specular\n\
misanthrope misanthropist\n\
misanthropic misanthropical\n\
misbranded mislabeled\n\
miscellaneous multifaceted multifarious\n\
mischance mishap\n\
miscible mixable\n\
miscreant reprobate\n\
misdirect misguide mislead\n\
misguided mistaken\n\
mislaid misplaced\n\
mislay misplace\n\
mismatched uneven\n\
mismated unsuited\n\
misogynistic misogynous\n\
missile projectile\n\
missional missionary\n\
missionary missioner\n\
missis missus\n\
mississippi ms\n\
missouri mo\n\
mistress schoolmarm schoolmistress\n\
miter mitre\n\
miterwort mitrewort\n\
mithra mithras\n\
mithracin mithramycin\n\
mithraic mithraistic\n\
mitomycin mutamycin\n\
mix premix\n\
mix shuffle\n\
mizen mizenmast mizzen mizzenmast\n\
mizen mizzen\n\
mnemonic mnemotechnic mnemotechnical\n\
mo molybdenum\n\
moban molindone\n\
mobbish moblike\n\
mobile nomadic peregrine roving wandering\n\
mocambique mozambique\n\
mocassin moccasin\n\
mocker mockingbird\n\
mocking quizzical teasing\n\
mod modern modernistic\n\
model poser\n\
model simulation\n\
modeled sculptural sculptured sculpturesque\n\
modeler modeller\n\
modeling mold molding mould moulding\n\
moderate restrained\n\
moderate temperate\n\
moderateness moderation\n\
moderateness modestness reasonableness\n\
moderation temperance\n\
modernised modernized\n\
modestness modesty\n\
modesty reserve\n\
mogadiscio mogadishu\n\
moghul mogul\n\
mohammedan muhammadan\n\
mohammedan muhammadan muhammedan\n\
mohave mojave\n\
moirae moirai\n\
moire watered\n\
mol mole\n\
mold mould\n\
moldavia moldova\n\
moldboard mouldboard\n\
molded shaped wrought\n\
moldiness must mustiness\n\
molding moulding\n\
moldy mouldy musty\n\
molech moloch\n\
mollah mulla mullah\n\
mollie molly\n\
mollusc mollusk shellfish\n\
molotov perm\n\
molter moulter\n\
momos momus\n\
momot motmot\n\
monacan monegasque\n\
monad monas\n\
monal monaul\n\
monarch sovereign\n\
monarchal monarchic monarchical\n\
monarchal monarchical\n\
monarchist royalist\n\
monastic monk\n\
monatomic monoatomic\n\
monecious monoecious monoicous\n\
monera prokayotae\n\
moneran moneron\n\
monestrous monoestrous\n\
monetary pecuniary\n\
moneyed monied\n\
moneylender shylock usurer\n\
monggo moong mung munggo\n\
mongol mongolian\n\
mongrelise mongrelize\n\
monitor proctor\n\
monitor varan\n\
monkey potter putter tinker\n\
monkeypod saman zaman zamang\n\
mono monophonic\n\
monochromatic monochrome monochromic monochromous\n\
monocycle unicycle\n\
monodic monodical\n\
monogamist monogynist\n\
monogynic monogynous\n\
mononuclear mononucleate\n\
monophysite monophysitic\n\
monopoliser monopolist monopolizer\n\
monosaccharide monosaccharose\n\
monotone monotonic\n\
monotone monotonic monotonous\n\
monovalent univalent\n\
monster teras\n\
monstrance ostensorium\n\
montana mt\n\
monument repository\n\
moody temperamental\n\
moon moonlight moonshine\n\
moonfish opah\n\
moonlit moony\n\
moor moorland\n\
moorbird moorfowl moorgame\n\
moorish moresque\n\
mop swab swob\n\
morbidity morbidness unwholesomeness\n\
mordva mordvin mordvinian\n\
morgue mortuary\n\
moribund stagnant\n\
moroseness sourness sulkiness sullenness\n\
morphia morphine\n\
morphologic morphological\n\
morphologic morphological structural\n\
morrigan morrigu\n\
mortgager mortgagor\n\
mortice mortise\n\
mortician undertaker\n\
mosaic photomosaic\n\
mosh slam\n\
moslem muslim\n\
motif motive\n\
motion movement\n\
motivating motivative motive\n\
motivation motive need\n\
motive motor\n\
motiveless unprovoked wanton\n\
motorboat powerboat\n\
motored motorised motorized\n\
motorless unmotorised unmotorized\n\
motortruck truck\n\
moufflon mouflon\n\
moujik mujik muzhik muzjik\n\
mountainside versant\n\
mournful plaintive\n\
mouselike mousey mousy\n\
mousey mousy\n\
moussorgsky mussorgsky\n\
moustache mustache\n\
moustachio mustachio\n\
mouth mouthpiece\n\
mouthful taste\n\
mouton mutton\n\
movability movableness\n\
movable moveable transferable transferrable transportable\n\
mover proposer\n\
mozartean mozartian\n\
mt tonne\n\
muadhdhin muazzin muezzin\n\
muckraker mudslinger\n\
mucky muddy\n\
mucoid mucoidal\n\
mucose mucous\n\
muddle puddle\n\
mudskipper mudspringer\n\
muenchen munich\n\
muffle repress smother stifle strangle\n\
muffled muted softened\n\
muffler silencer\n\
mug mugful\n\
muggy steamy sticky\n\
mule scuff\n\
muleteer skinner\n\
mulishness obstinacy obstinance stubbornness\n\
muller muser ponderer ruminator\n\
muller pestle pounder\n\
muller regiomontanus\n\
multinational transnational\n\
multinomial polynomial\n\
multiphase polyphase\n\
multiplicity numerosity numerousness\n\
multistorey multistoried multistory\n\
multivalent polyvalent\n\
mumbler murmurer mutterer\n\
munchausen munchhausen\n\
mundane terrene\n\
mundane terrestrial\n\
mundaneness mundanity ordinariness\n\
mundaneness mundanity sophistication worldliness\n\
munition ordnance\n\
munj munja\n\
munro saki\n\
murmuring susurrant whispering\n\
murmurous rustling soughing susurrous\n\
murphy potato spud tater\n\
muscadel muscadelle muscat muscatel\n\
muscat muscatel\n\
muscat muskat\n\
muscle muscleman\n\
muscle musculus\n\
mush pulp\n\
mushiness pulpiness\n\
musicality musicalness\n\
muskhogean muskogean\n\
muskrat musquash\n\
muss tussle\n\
mustached mustachioed\n\
mustelid musteline\n\
muster rally summon\n\
mutability mutableness\n\
mutant mutation sport variation\n\
mute tongueless unspoken wordless\n\
muteness silence\n\
mutual reciprocal\n\
muztag muztagh\n\
mycobacteria mycobacterium\n\
mycophage mycophagist\n\
mycostatin nystan nystatin\n\
myelic myeloid\n\
myg myriagram\n\
mym myriameter myriametre\n\
myofibril myofibrilla sarcostyle\n\
myopic nearsighted shortsighted\n\
myopic shortsighted unforesightful\n\
myrcia myrciaria\n\
myrtales thymelaeales\n\
mysoline primidone\n\
mysterious mystic mystical occult orphic\n\
mystic mystical\n\
myxobacter myxobacteria myxobacterium\n\
myxobacterales myxobacteriales\n\
myxobacteriaceae polyangiaceae\n\
myxophyceae schizophyceae\n\
na sodium\n\
naan nan\n\
nabob nawab\n\
nabu nebo\n\
nabumetone relafen\n\
nafcil nafcillin\n\
nafud nefud\n\
nag nagger scold scolder\n\
nagging shrewish\n\
naiadaceae najadaceae\n\
naiant swimming\n\
naias najas\n\
naif naive\n\
naive primitive\n\
naive unenlightened uninstructed\n\
naive uninitiate uninitiated\n\
naiveness naivete naivety\n\
najd nejd\n\
naked nude\n\
nakedness openness\n\
nalline nalorphine\n\
naloxone narcan\n\
nameless unidentified unknown unnamed\n\
nammad numdah\n\
namtar namtaru\n\
nandu rhea\n\
nanjing nanking\n\
nanny nurse nursemaid\n\
nanogram ng\n\
naomi noemi\n\
nape nucha scruff\n\
naphazoline privine sudafed\n\
napkin serviette\n\
naples napoli\n\
naprosyn naproxen\n\
naqua trichlormethiazide\n\
narc nark\n\
narcism narcissism\n\
narcissist narcist\n\
narcotic narcotising narcotizing\n\
narcotic soporiferous soporific\n\
nard spikenard\n\
nardil phenelzine\n\
nardo nardoo\n\
narrator storyteller teller\n\
narrowing tapered tapering\n\
narrowness slimness\n\
narwal narwhal narwhale\n\
nasal rhinal\n\
nationalism patriotism\n\
nationalist nationalistic\n\
nationalist patriot\n\
nativist nativistic\n\
naturalisation naturalization\n\
naturalised naturalized\n\
naturalistic realistic\n\
naturist nudist\n\
naumachia naumachy\n\
nauseated nauseous queasy sickish\n\
navaho navajo\n\
navane thiothixene\n\
navicular scaphoid\n\
navigate pilot\n\
navigate sail voyage\n\
nb niobium\n\
nd neodymium\n\
ne nebraska\n\
ne neon\n\
neandertal neanderthal\n\
neandertal neanderthal neanderthalian\n\
neat orderly\n\
neatness tidiness\n\
neb snout\n\
nebbech nebbish\n\
nebcin tobramycin\n\
nebuchadnezzar nebuchadrezzar\n\
nebular nebulous\n\
nebulous unfixed\n\
necromantic necromantical\n\
need want\n\
needed needful required requisite\n\
needer wanter\n\
needlecraft needlework\n\
needlefish pipefish\n\
neencephalon neoencephalon\n\
nefarious villainous\n\
nefariousness ugliness vileness wickedness\n\
nefazodone serzone\n\
negativeness negativism negativity\n\
negativeness negativity\n\
neglect neglectfulness negligence\n\
neglected unattended\n\
negligible paltry trifling\n\
negociate negotiate\n\
negotiant negotiator treater\n\
negotiatress negotiatrix\n\
negro negroid\n\
neighbor neighbour\n\
neighborhood region\n\
neighborliness neighbourliness\n\
neighborly neighbourly\n\
nelfinavir viracept\n\
nematode roundworm\n\
nembutal pentobarbital\n\
nemertea nemertina\n\
nemertean nemertine\n\
neoclassic neoclassical\n\
neoclassicist neoclassicistic\n\
neocon neoconservative\n\
neocortex neopallium\n\
neonate newborn\n\
neostigmine prostigmin\n\
neotenic neotenous\n\
nepalese nepali\n\
nepheline nephelite\n\
nephritic renal\n\
neptunium np\n\
neruda reyes\n\
nerve nervus\n\
nervous neural\n\
nervure vein\n\
nescient unbelieving\n\
nestle snuggle\n\
nestled snuggled\n\
net nett\n\
nether under\n\
neural neuronal neuronic\n\
neurilemma neurolemma\n\
neuroanatomic neuroanatomical\n\
neurologic neurological\n\
neurolysin neurotoxin\n\
neuropil neuropile\n\
neuropteran neuropteron\n\
neurotic psychoneurotic\n\
neuter sexless\n\
neutralised neutralized\n\
neutrophil neutrophile\n\
nevada nv\n\
nevirapine viramune\n\
news newsworthiness\n\
newsagent newsdealer newsvendor\n\
newsman newsperson reporter\n\
newspaper newsprint\n\
newspaper paper\n\
newt triton\n\
ni nickel\n\
nibble nybble\n\
nicaean nicene\n\
nice skillful\n\
niceness politeness\n\
niceness subtlety\n\
niche recess\n\
nick snick\n\
nidaros trondheim\n\
nifedipine procardia\n\
nigerian nigerien\n\
night nox\n\
nightbird nighthawk\n\
nightclothes nightwear sleepwear\n\
nightlong overnight\n\
nilgai nylghai nylghau\n\
nincompoop ninny poop\n\
ninefold nonuple\n\
ninepin skittle\n\
ninhursag ninkharsag ninkhursag\n\
ninib ninurta\n\
nintoo nintu\n\
nipa nypa\n\
niter nitre saltpeter saltpetre\n\
nitramine tetryl\n\
nitroglycerin nitroglycerine nitrospan nitrostat trinitroglycerin\n\
nitrospan nitrostat\n\
nitweed pineweed\n\
nitwitted senseless witless\n\
njord njorth\n\
no nobelium\n\
nob toff\n\
nock score\n\
noctambulist sleepwalker somnambulist\n\
nodular nodulated noduled\n\
nodule tubercle\n\
nog peg\n\
noisiness racketiness\n\
nominal nominative\n\
nominal titular\n\
nominal token tokenish\n\
nominated nominative\n\
nomogram nomograph\n\
nonabsorbent nonabsorptive\n\
nonachiever underachiever underperformer\n\
nonadsorbent nonadsorptive\n\
nonaggressive unaggressive\n\
nonarbitrary unarbitrary\n\
nonattender truant\n\
nonautonomous nonsovereign\n\
noncarbonated uncarbonated\n\
noncausal noncausative\n\
noncivilised noncivilized\n\
noncollapsable noncollapsible\n\
noncolumned uncolumned\n\
noncommunicable noncontagious nontransmissible\n\
nonconducting nonconductive\n\
nonconforming nonconformist\n\
nonconformist recusant\n\
nonconformist unconformist\n\
noncontroversial uncontroversial\n\
nonconvergent nonintersecting\n\
noncritical noncrucial\n\
noncritical uncritical\n\
noncyclic noncyclical\n\
nonelected nonelective\n\
nonenterprising unenterprising\n\
nonexempt taxable\n\
nonexplorative nonexploratory unexplorative unexploratory\n\
nonflavored nonflavoured unflavored unflavoured\n\
nonglutinous nonviscid\n\
nongregarious nonsocial\n\
nonhereditary nontransmissible\n\
nonheritable noninheritable\n\
nonhierarchic nonhierarchical\n\
nonindulgent strict\n\
noninstitutionalised noninstitutionalized\n\
nonintegrated unintegrated\n\
nonionic nonionised nonionized unionised unionized\n\
nonionic nonpolar\n\
nonkosher terefah tref\n\
nonliterary unliterary\n\
nonliterate preliterate\n\
nonmandatory nonobligatory\n\
nonmechanical unmechanical\n\
nonmedicinal unmedical unmedicative unmedicinal\n\
nonmetal nonmetallic\n\
nonmigratory resident\n\
nonmilitary unmilitary\n\
nonmoving unmoving\n\
nonmusical unmusical\n\
nonnatural otherworldly preternatural transcendental\n\
nonparallel serial\n\
nonparasitic nonsymbiotic\n\
nonpartisan nonpartizan\n\
nonperson unperson\n\
nonplused nonplussed puzzled\n\
nonpoisonous nontoxic\n\
nonrecreational paid\n\
nonreflecting nonreflective\n\
nonrenewable unrenewable\n\
nonrepresentative unsymbolic\n\
nonresinous nonresiny\n\
nonresonant unreverberant\n\
nonretractable nonretractile\n\
nonsectarian unsectarian\n\
nonsegmental unsegmented\n\
nonsense nonsensical\n\
nonsensitive unrestricted\n\
nonsteroid nonsteroidal\n\
nonsubjective objective\n\
nonsubmergible nonsubmersible\n\
nonsweet sugarless\n\
nonsyllabic unsyllabic\n\
nonsynchronous unsynchronised unsynchronized unsynchronous\n\
nontechnical untechnical\n\
nontelescopic nontelescoping\n\
nontraditional untraditional\n\
nontransferable unassignable untransferable\n\
nonunionised nonunionized unorganised unorganized\n\
nonviolent unbloody\n\
nonvolatile nonvolatilisable nonvolatilizable\n\
noradrenaline norepinephrine\n\
noreaster northeaster\n\
noreg norge norway\n\
norethandrolone norethindrone norlutin\n\
norflex orphenadrine\n\
normalcy normality\n\
normaliser normalizer\n\
normandie normandy\n\
normative prescriptive\n\
norse norseman norwegian\n\
norse northman scandinavian\n\
norse norwegian\n\
norse scandinavian\n\
north union\n\
northbound northward\n\
northeast northeasterly\n\
northeast northeasterly northeastern\n\
northerly northern\n\
northerner yank yankee\n\
northernmost northmost\n\
northwest northwesterly\n\
northwest northwesterly northwestern\n\
nortriptyline pamelor\n\
norvir ritonavir\n\
nose nozzle\n\
nose nuzzle\n\
noseband nosepiece\n\
nosey nosy prying snoopy\n\
nosher snacker\n\
notable noteworthy remarkable\n\
notched serrate serrated toothed\n\
noticeability noticeableness obviousness patency\n\
notional speculative\n\
notornis takahe\n\
novel refreshing\n\
novice novitiate\n\
novocain novocaine\n\
nowness presentness\n\
nub nubble\n\
nub stub\n\
nucleate nucleated\n\
nucleole nucleolus\n\
nudge prod\n\
nudnick nudnik\n\
nuisance pain\n\
null void\n\
numeral numeric numerical\n\
numeric numerical\n\
numididae numidinae\n\
numismatist numismatologist\n\
nuremberg nurnberg\n\
nursed suckled\n\
nurseling nursling suckling\n\
nurture raising rearing\n\
nutcracker nuthatch\n\
nutgrass nutsedge\n\
nutlike nutty\n\
nutritional nutritionary\n\
nutritiousness nutritiveness\n\
nutter wacko whacko\n\
nylons rayons\n\
nympho nymphomaniac\n\
nymphomaniac nymphomaniacal\n\
oarfish ribbonfish\n\
oarlock peg rowlock thole tholepin\n\
oarsman rower\n\
objectionable obnoxious\n\
objectiveness objectivity\n\
obliterable removable\n\
obliterate obliterated\n\
oblivious unmindful\n\
oblongness rectangularity\n\
obsequiousness servility subservience\n\
observant observing\n\
obsessed possessed\n\
obsessional obsessive\n\
obsessiveness obsessivity\n\
obsoleteness superannuation\n\
obstetric obstetrical\n\
obstinate stubborn unregenerate\n\
obstructer obstructionist obstructor resister thwarter\n\
obtuse purblind\n\
obviating preclusive\n\
oca oka\n\
occam ockham\n\
occasional periodic\n\
occident west\n\
occluded sorbed\n\
occult supernatural\n\
occupant occupier resident\n\
occupied tenanted\n\
ocean sea\n\
oceangoing seafaring seagoing\n\
oceania oceanica\n\
oceanic pelagic\n\
ocellus stemma\n\
ocher ochre\n\
octagonal octangular\n\
octoberfest oktoberfest\n\
ocular ophthalmic optic optical\n\
ocular optic optical visual\n\
ocular visual\n\
oculist ophthalmologist\n\
oculist optometrist\n\
odd uneven\n\
odd unmatched unmated unpaired\n\
oddity oddness\n\
odesa odessa\n\
odo otho\n\
odoacer odovacar odovakar\n\
odoriferous odorous\n\
odoriferous odorous perfumed scented\n\
off turned\n\
offenceless offenseless\n\
offender wrongdoer\n\
offensive unsavory unsavoury\n\
offensive violative\n\
offerer offeror\n\
offhand offhanded\n\
officeholder officer\n\
officer policeman\n\
official prescribed\n\
offish standoffish\n\
offload unlade unload\n\
offsaddle unsaddle\n\
offset runner stolon\n\
offset setoff\n\
offshore seaward\n\
offside offsides\n\
ogalala oglala\n\
oh ohio\n\
oil petroleum\n\
oiler tanker\n\
oilskin slicker\n\
ok oklahoma\n\
oken okenfuss\n\
oklahoman sooner\n\
ola olla\n\
old older\n\
old previous\n\
oldtimer stager veteran warhorse\n\
oldwench oldwife\n\
olein triolein\n\
olfactive olfactory\n\
oligarchic oligarchical\n\
oligo oligonucleotide\n\
oligoclase plagioclase\n\
oligodendria oligodendroglia\n\
olimbos olympus\n\
olympian olympic\n\
omasum psalterium\n\
omelet omelette\n\
omeprazole prilosec\n\
omnipresent ubiquitous\n\
oncologic oncological\n\
oncovin vincristine\n\
oneness unity\n\
onomatopoeic onomatopoetic\n\
onopordon onopordum\n\
onychophoran peripatus\n\
ooze seep\n\
oozing oozy seeping\n\
opacity opaqueness\n\
opaque unintelligible\n\
opencast opencut\n\
opener undoer unfastener untier\n\
opening orifice porta\n\
openmouthed popeyed\n\
openness receptiveness receptivity\n\
operable practicable\n\
operating operational\n\
operative pi shamus sherlock\n\
operative surgical\n\
operculate operculated\n\
ophidia serpentes\n\
ophidian serpent snake\n\
opiliones phalangida\n\
opinionated opinionative\n\
oporto porto\n\
opossum phalanger possum\n\
opossum possum\n\
opponent opposing\n\
opponent opposite opposition\n\
opportuneness patness timeliness\n\
opportunist opportunistic timeserving\n\
opposite paired\n\
oppressive tyrannical tyrannous\n\
optimal optimum\n\
opv topv\n\
or oregon\n\
or surgery\n\
orach orache\n\
oracle prophesier prophet seer vaticinator\n\
oral unwritten\n\
orang orangutan orangutang\n\
orange orangeness\n\
orange orangish\n\
orator rhetorician speechifier speechmaker\n\
orb orbit revolve\n\
orbicular orbiculate\n\
orbiter satellite\n\
orderer systematiser systematist systematizer systemiser systemizer\n\
oreide oroide\n\
organdie organdy\n\
organisation organization system\n\
organisational organizational\n\
organised organized unionised unionized\n\
organiser organizer\n\
organiser organizer pda\n\
organismal organismic\n\
orientated oriented\n\
orientating orienting\n\
orinase tolbutamide\n\
ormazd ormuzd\n\
ornithopter orthopter\n\
orotund rotund\n\
orris orrisroot\n\
orthogonal rectangular\n\
orthopaedic orthopedic orthopedical\n\
orthopaedist orthopedist\n\
orthophosphate phosphate\n\
orthopteran orthopteron\n\
orumiyeh urmia\n\
oryx pasang\n\
os osmium\n\
oscillate vibrate\n\
oscillating oscillatory\n\
oscines passeres\n\
osmanli ottoman\n\
ossicular ossiculate\n\
ostensible ostensive\n\
ostentatious pretentious\n\
osteologer osteologist\n\
osteopath osteopathist\n\
otherworldliness spiritism spiritualism spirituality\n\
otiose pointless purposeless senseless superfluous wasted\n\
oto otoe\n\
otolaryngologist otorhinolaryngologist rhinolaryngologist\n\
ottawa outaouais\n\
ouranos uranus\n\
outback remote\n\
outbound outward\n\
outcrop outcropping\n\
outdated superannuated\n\
outermost outmost\n\
outerwear overclothes\n\
outpost outstation\n\
output outturn turnout\n\
output production yield\n\
outright unlimited\n\
outsize outsized oversize oversized\n\
outspoken vocal\n\
outstanding owing undischarged\n\
outstanding prominent salient spectacular striking\n\
overabundance overmuch overmuchness superabundance\n\
overabundant plethoric rife\n\
overage overaged superannuated\n\
overarm overhand overhanded\n\
overburden overload\n\
overcast overcasting\n\
overcharge overload surcharge\n\
overcoat overcoating\n\
overcomer subduer surmounter\n\
overemotional sloppy\n\
overflow overrun\n\
overhand oversewn\n\
overhaul overtake\n\
overhead viewgraph\n\
overladen overloaded\n\
overlay overlayer sheathing\n\
overlay overlie\n\
overleap vault\n\
overlooked unmarked unnoted\n\
overlying superimposed\n\
overpowering overwhelming\n\
overprint surprint\n\
overreaching vaulting\n\
overrefined superfine\n\
overriding paramount predominant predominate preponderant preponderating\n\
oversea overseas\n\
overseer superintendent\n\
overstep transgress trespass\n\
overturned upturned\n\
overweening uppity\n\
ovolo thumb\n\
owner possessor\n\
owner proprietor\n\
ownerless unowned\n\
oxalacetate oxaloacetate\n\
oxalis sorrel\n\
oxazepam serax\n\
oxidant oxidiser oxidizer\n\
oxidised oxidized\n\
oxlip paigle\n\
oxyhaemoglobin oxyhemoglobin\n\
oxyphenbutazone tandearil\n\
oxytocin pitocin\n\
ozocerite ozokerite\n\
pa pascal\n\
pa pennsylvania\n\
pa protactinium protoactinium\n\
pace rate\n\
pace yard\n\
pacemaker pacer pacesetter\n\
pacha pasha\n\
pachouli patchouli patchouly\n\
pachycephalosaur pachycephalosaurus\n\
pachydermal pachydermatous pachydermic pachydermous\n\
pacific peaceable\n\
package parcel\n\
packing wadding\n\
paddymelon pademelon\n\
padova padua patavium\n\
paederast pederast\n\
paederastic pederastic\n\
paediatric pediatric\n\
paediatrician pediatrician pediatrist\n\
paedophile pedophile\n\
paeony peony\n\
page pageboy\n\
page varlet\n\
pahlavi pahlevi\n\
pail pailful\n\
paillasse palliasse\n\
paint pigment\n\
painting picture\n\
paiute piute\n\
pajama pyjama\n\
palaeencephalon paleencephalon paleoencephalon\n\
palaeolithic paleolithic\n\
palaeontological paleontological\n\
palaestra palestra\n\
palankeen palanquin\n\
palatability palatableness\n\
palatable toothsome\n\
palatal palatalised palatalized\n\
palatal palatine\n\
palatinate pfalz\n\
palatine palsgrave\n\
palau tt\n\
pale pallid\n\
pale pallid wan\n\
pale picket\n\
paleness pallidity\n\
paleographer paleographist\n\
paleostriatum pallidum\n\
palette pallet\n\
palette pallette\n\
palladium pd\n\
palm thenar\n\
palmar volar\n\
palooka stumblebum\n\
palpability tangibility tangibleness\n\
palpable tangible\n\
palpitant palpitating\n\
palpitate quake quiver\n\
paltriness sorriness\n\
panamica panamiga\n\
panatela panetela panetella\n\
pandar pander panderer pimp ponce procurer\n\
pandurate panduriform\n\
pane paneling panelling\n\
paneled wainscoted\n\
panelist panellist\n\
pangaea pangea\n\
panjabi punjabi\n\
panocha panoche penoche penuche\n\
panoptic panoptical\n\
panpipe syrinx\n\
pant trousers\n\
pantheist pantheistic\n\
pantie panty scanty\n\
panting trousering\n\
pantropic pantropical\n\
papaia papaya pawpaw\n\
papaverales rhoeadales\n\
papaw pawpaw\n\
paper wallpaper\n\
paperback paperbacked\n\
paperback softback\n\
paperboard posterboard\n\
paperer paperhanger\n\
papillary papillose\n\
papist papistic papistical popish roman romanist romish\n\
papoose pappoose\n\
papooseroot squawroot\n\
paprika pimento pimiento\n\
para paratrooper\n\
parabolic parabolical\n\
parachuter parachutist\n\
parade promenade troop\n\
paradisaic paradisaical paradisal paradisiac paradisiacal\n\
parakeet paraquet paroquet parrakeet parroket parroquet\n\
parallelepiped parallelepipedon parallelopiped parallelopipedon\n\
paralytic paralytical\n\
paralytic paralyzed\n\
paramecia paramecium\n\
paramedic paramedical\n\
paranoiac paranoid\n\
parasitic parasitical\n\
parasol sunshade\n\
parazoan poriferan sponge\n\
parcel tract\n\
pare peel\n\
pare whittle\n\
parenthetic parenthetical\n\
parentless unparented\n\
pareve parve\n\
parget pargeting pargetting\n\
pargeting pargetry pargetting\n\
parheliacal parhelic\n\
parhelion sundog\n\
paries wall\n\
paring shaving sliver\n\
park parkland\n\
parlor parlour\n\
parlormaid parlourmaid\n\
parlous perilous precarious\n\
parnahiba parnaiba\n\
parolee probationer\n\
paroxetime paxil\n\
parqueterie parquetry\n\
parrotfish pollyfish\n\
parsec secpar\n\
parsee parsi\n\
parsimonious penurious\n\
parsimoniousness parsimony thrift\n\
parsonage rectory vicarage\n\
partaker sharer\n\
participant player\n\
particular peculiar\n\
particularised particularized\n\
particularity specialness\n\
partisan partizan\n\
partitive separative\n\
partner spouse\n\
partridge tinamou\n\
parvenu parvenue\n\
parvenu parvenue upstart\n\
parvo parvovirus\n\
paseo walk walkway\n\
pashtoon pashtun pathan pushtun\n\
passage passageway\n\
passementerie trimming\n\
passenger rider\n\
passer passerby\n\
passive peaceful\n\
passiveness passivity\n\
passport recommendation\n\
past preceding retiring\n\
pastelike pasty\n\
pasteurised pasteurized\n\
pastil pastille troche\n\
patchboard plugboard switchboard\n\
patched spotted spotty\n\
pate poll\n\
paterfamilias patriarch\n\
pathetic pitiable pitiful\n\
pathetic ridiculous silly\n\
pathfinder scout\n\
pathless roadless trackless untracked untrod untrodden\n\
pathologic pathological\n\
pathos poignancy\n\
pathway tract\n\
patinate patinise patinize\n\
patio terrace\n\
patrai patras\n\
patrilineal patrilinear\n\
patristic patristical\n\
patron sponsor\n\
patroness patronne\n\
patronised patronized\n\
patronless unpatronised unpatronized\n\
paul saul\n\
pavement paving\n\
pavement sidewalk\n\
pavior paviour\n\
pavis pavise\n\
payer remunerator\n\
payload warhead\n\
pb pbit petabit\n\
pb pebibyte petabyte pib\n\
pb petabyte\n\
pcp phencyclidine\n\
pdl poundal\n\
peaceable peaceful\n\
peag wampum wampumpeag\n\
peaky spiky\n\
pearlweed pearlwort\n\
peavey peavy\n\
pebibit pibit\n\
peccable peccant\n\
peck smack\n\
pecker peckerwood woodpecker\n\
pecs pectoral pectoralis\n\
pectoral thoracic\n\
pedagogic pedagogical\n\
pedal treadle\n\
pedaler pedaller\n\
pedestal stand\n\
pediapred prednisolone prelone\n\
pedicel pedicle\n\
pedigree pedigreed pureblood pureblooded thoroughbred\n\
pedipalpi uropygi\n\
pedunculate stalked\n\
pee piddle piss urine water weewee\n\
peeper voyeur\n\
peepul pipal pipul\n\
peewee peewit pewee pewit\n\
peewee runt shrimp\n\
peireskia pereskia\n\
peke pekinese pekingese\n\
pel pixel\n\
peloponnese peloponnesus\n\
pelting rain\n\
peludo poyou\n\
pemican pemmican\n\
pen penitentiary\n\
pen playpen\n\
penal punishable\n\
penciled pencilled\n\
pendant pendent\n\
peneplain peneplane\n\
penetrability perviousness\n\
penetrate perforate\n\
penetrating penetrative\n\
penial penile\n\
penitent repentant\n\
penitential penitentiary\n\
penman scribbler scribe\n\
penn pennsylvania\n\
pennant pennon streamer waft\n\
pennon pinion\n\
pennoncel pennoncelle penoncel\n\
pennywhistle whistle\n\
pensionary pensioner\n\
pensive wistful\n\
penstock sluice sluiceway\n\
pentacle pentagram pentangle\n\
pentaerythritol peritrate\n\
pentagonal pentangular\n\
pentazocine talwin\n\
pentecostal pentecostalist\n\
pentothal thiopental\n\
pentoxifylline trental\n\
peplos peplum peplus\n\
pepper peppercorn\n\
peptidase protease proteinase\n\
perceived sensed\n\
perch pole rod\n\
perch rest roost\n\
perchloromethane tetrachloromethane\n\
perciformes percomorphi\n\
percoid percoidean\n\
percussor plessor plexor\n\
percutaneous transcutaneous transdermal transdermic\n\
perdicidae perdicinae\n\
perennial recurrent repeated\n\
perfidious punic treacherous\n\
perfidiousness perfidy treachery\n\
perfluorocarbon pfc\n\
perforate perforated pierced punctured\n\
perforate punch\n\
perfumed scented\n\
pericardiac pericardial\n\
perilune periselene\n\
perinasal perirhinal\n\
periodic periodical\n\
periodontal periodontic\n\
peripatetic wayfaring\n\
perishability perishableness\n\
perishable spoilable\n\
peristylar pseudoperipteral\n\
periwig peruke\n\
periwigged peruked\n\
periwinkle winkle\n\
perm permanent\n\
permanence permanency\n\
permeability permeableness\n\
permeant permeating permeative pervasive\n\
permissiveness tolerance\n\
permutability permutableness transposability\n\
permutable transposable\n\
pernambuco recife\n\
perniciousness toxicity\n\
pernickety persnickety\n\
perpendicular vertical\n\
perpetuity sempiternity\n\
perphenazine triavil\n\
perquisite prerogative privilege\n\
persecutor tormenter tormentor\n\
persistent relentless unrelenting\n\
perspicacious sagacious sapient\n\
perspicuity perspicuousness plainness\n\
perspiration sudor sweat\n\
perspirer sweater\n\
perverseness perversity\n\
pessimal pessimum\n\
pestiferous pestilent pestilential plaguey\n\
petaled petalled petalous\n\
petitioner requester suppliant supplicant\n\
petitioner suer\n\
petrarca petrarch\n\
petrous stonelike\n\
petticoat underskirt\n\
pettifogger shyster\n\
pettiness puniness slightness triviality\n\
phacelia scorpionweed\n\
phaeton tourer\n\
phallic priapic\n\
phanerogam spermatophyte\n\
phantasmagoric phantasmagorical surreal surrealistic\n\
pharisaic pharisaical pietistic pietistical sanctimonious\n\
pharmaceutic pharmaceutical\n\
pharmacologic pharmacological\n\
pharynx throat\n\
phasmatidae phasmidae\n\
phasmatodea phasmida\n\
pheidias phidias\n\
phenazopyridine pyridium\n\
phenergan promethazine\n\
phenicia phoenicia\n\
phenolic phenoplast\n\
phenothiazine thiodiphenylamine\n\
phenotypic phenotypical\n\
phentolamine vasomax\n\
philanderer womaniser womanizer\n\
philatelic philatelical\n\
philippopolis plovdiv\n\
phillidae phyllidae\n\
philologist philologue\n\
philosophic philosophical\n\
philosophiser philosophizer\n\
philter philtre\n\
phintias pythias\n\
phlebogram venogram\n\
phlegm sputum\n\
phlegmatic phlegmatical\n\
phone telephone\n\
phonetic phonic\n\
phonologic phonological\n\
phoronida phoronidea\n\
phosphoric phosphorous\n\
photoconduction photoconductivity\n\
photoelectric photoelectrical\n\
photometric photometrical\n\
photometrician photometrist\n\
phragmacone phragmocone\n\
phthirius phthirus\n\
phyletic phylogenetic\n\
phylloquinone phytonadione\n\
phyllostomatidae phyllostomidae\n\
physiologic physiological\n\
phytophagic phytophagous phytophilous\n\
pianissimo piano\n\
piano pianoforte\n\
piaster piastre\n\
piazza plaza\n\
picaninny piccaninny pickaninny\n\
picardie picardy\n\
pickax pickaxe\n\
pickerelweed wampee\n\
picklepuss pouter sourpuss\n\
picknicker picnicker\n\
pictorial pictural\n\
piecemeal stepwise\n\
piedmont piemonte\n\
pieplant rhubarb\n\
pier wharf wharfage\n\
pietism religionism religiosity religiousism\n\
pietistic pietistical\n\
piety piousness\n\
pig slob sloven\n\
pigboat sub submarine\n\
piggy piglet shoat shote\n\
pigman swineherd\n\
pigmy pygmy\n\
pigpen pigsty sty\n\
pigswill pigwash slop slops swill\n\
pilaf pilaff pilau pilaw\n\
pilary pilose pilous\n\
pilchard sardine\n\
pilferer snitcher\n\
piling spile stilt\n\
pillbox toque turban\n\
pillow rest\n\
pilothouse wheelhouse\n\
pilsen plzen\n\
pilsener pilsner\n\
pimento pimiento\n\
pincer tweezer\n\
pinch tweet twinge twitch\n\
pindolol visken\n\
pinfish squirrelfish\n\
pinion quill\n\
pinion shackle\n\
pink pinkish\n\
pink pinko\n\
pinkie pinky\n\
pinna pinnule\n\
pinnate pinnated\n\
pinnatiped pinniped\n\
pinon pinyon\n\
pinophytina pinopsida\n\
pinpoint speck\n\
pinworm threadworm\n\
pipage pipe piping\n\
pipe pipework\n\
pipe tube\n\
piperacillin pipracil\n\
piperin piperine\n\
pipet pipette\n\
pipistrel pipistrelle\n\
piquance piquancy piquantness\n\
piquance piquancy piquantness tang tanginess zest\n\
piquant salty\n\
piquant savory savoury zesty\n\
pirate plagiariser plagiarist plagiarizer\n\
pirogi piroshki pirozhki\n\
pisanosaur pisanosaurus\n\
piscatorial piscatory\n\
pisser urinator\n\
piston plunger\n\
pitchblende uraninite\n\
pitcher pitcherful\n\
pitchy resinous resiny tarry\n\
pithecellobium pithecolobium\n\
pithy sententious\n\
pitiless remorseless ruthless unpitying\n\
pitiless unkind\n\
pitilessness ruthlessness\n\
pitprop sprag\n\
pivot swivel\n\
pivotal polar\n\
pix pyx\n\
pixie pixy pyxie\n\
placeable recognisable recognizable\n\
placeholder procurator proxy\n\
placeman placeseeker\n\
placid tranquil unruffled\n\
placidity repose serenity tranquility tranquillity\n\
placoid platelike\n\
plagiarised plagiaristic plagiarized\n\
plaid tartan\n\
plait pleat\n\
planaria planarian\n\
planet satellite\n\
planetal planetary\n\
planetary terrestrial\n\
plangency resonance reverberance ringing sonority sonorousness vibrancy\n\
plant works\n\
plash pleach\n\
plash spatter splash splatter splosh swash\n\
plasm plasma\n\
plaster plasterwork\n\
plastered sealed\n\
plastered slicked\n\
plastic pliant\n\
plasticiser plasticizer\n\
platan sycamore\n\
plate plateful\n\
plate scale shell\n\
plateau tableland\n\
platelayer tracklayer\n\
platelet thrombocyte\n\
platinum pt\n\
platyrrhine platyrrhinian\n\
plausibility plausibleness\n\
playfellow playmate\n\
playgoer theatergoer theatregoer\n\
plaything toy\n\
pleasantness sweetness\n\
pleat plicate\n\
pleb plebeian\n\
plebeian unwashed vulgar\n\
plecopteran stonefly\n\
plectron plectrum\n\
pledge toast\n\
pledged sworn\n\
plenitude plenteousness plentifulness plentitude plenty\n\
pleomorphism polymorphism\n\
pleonastic redundant tautologic tautological\n\
pleopod swimmeret\n\
plesiosaur plesiosaurus\n\
plessimeter pleximeter\n\
plexiglas plexiglass\n\
plexus rete\n\
pliability pliancy pliantness suppleness\n\
pliancy pliantness suppleness\n\
plier plyer\n\
pliers plyers\n\
plodder slogger\n\
plodder slogger trudger\n\
plodder slowcoach slowpoke\n\
plotter schemer\n\
plough plow\n\
ploughboy plowboy\n\
ploughed plowed\n\
ploughman plower plowman\n\
ploughshare plowshare share\n\
ploughwright plowwright\n\
plumate plumed plumose\n\
plumb plummet\n\
plumbic plumbous\n\
plume preen\n\
plumed plumy\n\
plumelike plumy\n\
plumeria plumiera\n\
plummet plump\n\
plunger speculator\n\
plutocratic plutocratical\n\
plutonium pu\n\
pluviometer udometer\n\
plyboard plywood\n\
pm premier\n\
pm promethium\n\
pneumogastric vagal\n\
pneumogastric vagus\n\
pneumonic pulmonary pulmonic\n\
po polonium\n\
pock scar\n\
pocked pockmarked\n\
pocked pockmarked potholed\n\
pocket pouch\n\
pocket pouch sac\n\
pocket scoop\n\
pod seedpod\n\
podsol podzol\n\
poeciliid topminnow\n\
poetic poetical\n\
poetiser poetizer rhymer rhymester versifier\n\
poilu purloo\n\
pointillist pointillistic\n\
pointless unpointed\n\
poison toxicant\n\
poisonous toxicant\n\
poisonous venomous vicious\n\
poker salamander\n\
pol politician politico\n\
poland polska\n\
polarimeter polariscope\n\
polaris polestar\n\
polarisation polarization\n\
pole punt\n\
pole terminal\n\
poleax poleaxe\n\
polecat skunk\n\
polemic polemical\n\
polemic polemicist polemist\n\
polish shine smoothen\n\
polished refined urbane\n\
poll pollard\n\
pollack pollock\n\
pollex thumb\n\
polliwog pollywog tadpole\n\
polybotria polybotrya\n\
polybutene polybutylene\n\
polychaete polychete\n\
polychromatic polychrome polychromic\n\
polydactyl polydactylous\n\
polyestrous polyoestrous\n\
polyethylene polythene\n\
polymorphic polymorphous\n\
polyose polysaccharide\n\
polyphonic polyphonous\n\
polypropene polypropylene\n\
polysemantic polysemous\n\
polysyllabic sesquipedalian\n\
polytetrafluoroethylene teflon\n\
polyurethan polyurethane\n\
pom pommy\n\
pomade pomatum\n\
pomelo pummelo shaddock\n\
pomelo shaddock\n\
pommel saddlebow\n\
pompey portsmouth\n\
ponca ponka\n\
pond pool\n\
pontiff pope\n\
pontos pontus\n\
pool puddle\n\
poop quarter\n\
pop popular\n\
pop soda\n\
populariser popularizer vulgariser vulgarizer\n\
porc pork\n\
pore stoma stomate\n\
porgy scup\n\
poriferous porous\n\
porosity porousness\n\
portentous prodigious\n\
portly stout\n\
portrait portrayal\n\
poser poseur\n\
positiveness positivism positivity\n\
positiveness positivity\n\
positivist positivistic\n\
positivist rationalist\n\
possession willpower\n\
possible potential\n\
post stake\n\
post station\n\
posteriority subsequence subsequentness\n\
postilion postillion\n\
postmodern postmodernist\n\
postmortal postmortem\n\
postnatal postpartum\n\
postpaid prepaid\n\
potage pottage\n\
potbound rootbound\n\
potboy potman\n\
potemkin potyokin\n\
potent powerful\n\
potent virile\n\
potential voltage\n\
potholer spelaeologist speleologist spelunker\n\
pothouse pub saloon taphouse\n\
potter putter\n\
potterer putterer\n\
potty tiddly tipsy\n\
poulterer poultryman\n\
pounce swoop\n\
pour pullulate stream swarm teem\n\
powder pulverisation pulverization\n\
powdered powdery pulverised pulverized\n\
powhatan wahunsonacock\n\
pr praseodymium\n\
practicability practicableness\n\
practical virtual\n\
practiced practised\n\
practician practitioner\n\
praetor pretor\n\
praetorial praetorian pretorial pretorian\n\
praetorian pretorian\n\
praetorium pretorium\n\
prag prague praha\n\
pragmatic pragmatical\n\
pragmatism realism\n\
prance sashay strut swagger tittup\n\
prankishness rascality roguishness\n\
pravachol pravastatin\n\
prawn shrimp\n\
prayer supplicant\n\
preacher sermoniser sermonizer\n\
preadolescent preteen\n\
precarious shaky\n\
precarious unstable\n\
precariousness uncertainness uncertainty\n\
precative precatory\n\
precautional precautionary\n\
preciosity preciousness\n\
precious valued\n\
preciseness precision\n\
preclinical presymptomatic\n\
precursory premonitory\n\
predaceous predacious\n\
predaceous predacious predatory\n\
predatory rapacious raptorial ravening vulturine vulturous\n\
predetermined preset\n\
predictive prognostic prognosticative\n\
predominance predomination\n\
preemie premie\n\
preexistent preexisting\n\
preferable preferred\n\
prehistoric prehistorical\n\
prejudicial prejudicious\n\
premature previous\n\
premature untimely\n\
premier premiere\n\
premier prime\n\
prepackaged prepacked\n\
preparative preparatory propaedeutic\n\
preponderance prevalence\n\
prepubertal prepubescent\n\
presenter sponsor\n\
preserver refinisher renovator restorer\n\
president prexy\n\
pressing urgent\n\
pressman printer\n\
pressor vasoconstrictive vasoconstrictor\n\
presumable supposable surmisable\n\
preteen preteenager\n\
pretence pretense pretension\n\
preternatural uncanny\n\
preussen prussia\n\
preventative preventive\n\
preventative preventive prophylactic\n\
prey quarry\n\
prey quarry target\n\
pricker prickle spikelet spine sticker thorn\n\
prickleback stickleback\n\
priestlike priestly\n\
prig snob snoot snot\n\
priggish prim prissy prudish puritanical straightlaced straitlaced victorian\n\
priggishness primness\n\
prime undercoat\n\
primer priming undercoat\n\
primogenitor progenitor\n\
primrose primula\n\
princedom principality\n\
principal star\n\
privacy privateness seclusion\n\
privateer privateersman\n\
privy secluded\n\
prize trophy\n\
pro professional\n\
probationary provisional provisionary tentative\n\
probative probatory\n\
proboscidean proboscidian\n\
proboscis trunk\n\
procaryote prokaryote\n\
procaryotic prokaryotic\n\
process serve\n\
processed refined\n\
procurer securer\n\
prodigal profligate squanderer\n\
prodromal prodromic\n\
product production\n\
productiveness productivity\n\
proenzyme zymogen\n\
prof professor\n\
profane secular\n\
profane unconsecrated unsanctified\n\
profaned violated\n\
profaneness unsanctification\n\
proficient technical\n\
profound unfathomed unplumbed unsounded\n\
profound wakeless\n\
profoundness profundity\n\
progestin progestogen\n\
progressive reformist\n\
progressiveness progressivity\n\
prohibitive prohibitory\n\
projectile rocket\n\
prole proletarian worker\n\
promiscuous sluttish wanton\n\
promiser promisor\n\
promptitude promptness\n\
promptness punctuality\n\
promulgated published\n\
prone prostrate\n\
prongbuck pronghorn\n\
pronged tined\n\
prop property\n\
prop shore\n\
propagandist propagandistic\n\
propanal propionaldehyde\n\
propanamide proprionamide\n\
propellant propellent\n\
propellant propellent propelling propulsive\n\
propeller propellor\n\
propene propylene\n\
prophetic prophetical\n\
propinquity proximity\n\
propitiative propitiatory\n\
propjet turboprop\n\
proportion proportionality\n\
proportion symmetry\n\
proportional relative\n\
proposer suggester\n\
prosaicness prosiness\n\
proserpina proserpine\n\
prostate prostatic\n\
prostheon prosthion\n\
prostyle pseudoprostyle\n\
protected saved\n\
protirelin trf trh\n\
protist protistan\n\
protomammal therapsid\n\
protozoal protozoan protozoic\n\
protozoan protozoon\n\
protractible protractile\n\
protrusible protrusile\n\
proturan telsontail\n\
proved proven\n\
provider supplier\n\
province state\n\
provisioner sutler victualer victualler\n\
prowler sneak stalker\n\
prox proximo\n\
prude puritan\n\
pruner trimmer\n\
pseudohermaphrodite pseudohermaphroditic\n\
pseudopod pseudopodium\n\
pseudowintera wintera\n\
psilocin psilocybin\n\
psilopsida psilotatae\n\
psittacosaur psittacosaurus\n\
psyche soul\n\
psychiatric psychiatrical\n\
psychiatrist shrink\n\
psychic psychical\n\
psycho psychotic\n\
psychoactive psychotropic\n\
psychoanalytic psychoanalytical\n\
psychogenetic psychogenic\n\
psychopath sociopath\n\
psychopathic psychopathologic psychopathological\n\
psylla psyllid\n\
pteridospermae pteridospermaphyta\n\
ptomain ptomaine\n\
publicised publicized\n\
publiciser publicist publicizer\n\
pucka pukka\n\
pucker ruck\n\
pud pudding\n\
pulasan pulassan\n\
pullback tieback\n\
pullover slipover\n\
pulpy squashy\n\
pulsate pulse throb\n\
pulsate quiver\n\
pumped wired\n\
punch puncher\n\
puniness runtiness stuntedness\n\
punitive punitory\n\
punkey punkie punky\n\
puny runty shrimpy\n\
pup puppy\n\
pup whelp\n\
pupil schoolchild\n\
puppyish puppylike\n\
pure saturated\n\
pure vestal virgin virginal virtuous\n\
pureblood purebred thoroughbred\n\
puree strain\n\
purgatorial purging purifying\n\
puritanic puritanical\n\
purple purpleness\n\
purple purplish violet\n\
purse wrinkle\n\
purulent pussy\n\
pushful pushy\n\
pushpin thumbtack\n\
pusillanimity pusillanimousness\n\
pusillanimous unmanly\n\
pussley pussly verdolagas\n\
putrefacient putrefactive\n\
putrescence rottenness\n\
pyaemic pyemic\n\
pycnotic pyknotic\n\
pyralidae pyralididae\n\
pyramidal pyramidic pyramidical\n\
pyrectic pyrogen\n\
pyroelectric pyroelectrical\n\
pyrogenetic pyrogenic pyrogenous\n\
pyrola wintergreen\n\
pyroligneous pyrolignic\n\
pyrotechnic pyrotechnical\n\
pyroxylin pyroxyline\n\
pyrrhotine pyrrhotite\n\
pythia pythoness\n\
pyxidium pyxis\n\
qindarka qintar\n\
quackgrass witchgrass\n\
quad quadrangle\n\
quad quadriceps\n\
quad quadruplet\n\
quad space\n\
quadrangle quadrilateral tetragon\n\
quadraphonic quadrasonic quadriphonic quadrisonic\n\
quadruped quadrupedal\n\
quahaug quahog\n\
quake tremor\n\
quaker trembler\n\
qualified restricted\n\
quality timber timbre tone\n\
quandang quandong\n\
quandang quandong quantong\n\
quantal quantized\n\
quarreler quarreller\n\
quarrier quarryman\n\
quaternary quaternate\n\
quavering tremulous\n\
queasiness restlessness uneasiness\n\
queen tabby\n\
queenlike queenly\n\
quelled quenched squelched\n\
quenched satisfied slaked\n\
quenchless unquenchable\n\
quester searcher seeker\n\
questioning quizzical\n\
quietness soundlessness\n\
quin quint quintuplet\n\
quincentenary quincentennial\n\
quinidex quinidine quinora\n\
quixotic romantic\n\
quotable repeatable\n\
ra radium\n\
ra re\n\
rabato rebato\n\
rabbet rebate\n\
rabbinic rabbinical\n\
rabbitweed snakeweed\n\
raccoon racoon\n\
race raceway\n\
race rush\n\
racecourse racetrack raceway\n\
rachet ratch ratchet\n\
rachitic rickety\n\
rachmaninoff rachmaninov\n\
racialist racist\n\
rack scud\n\
rack stand\n\
rack wheel\n\
racket racquet\n\
rackety uproarious\n\
racking wrenching\n\
racy rich robust\n\
rad radian\n\
radar radiolocation\n\
raddle reddle ruddle\n\
raddle ruddle\n\
radial radiate stellate\n\
radical revolutionary\n\
radio tuner wireless\n\
radio wireless\n\
radiogram radiograph shadowgraph skiagram skiagraph\n\
radiologist radiotherapist\n\
radiophone radiotelephone\n\
radiophonic radiotelephonic\n\
radiophoto radiophotograph\n\
radiotelegraph radiotelegraphy\n\
radius spoke\n\
radon rn\n\
raffia raphia\n\
raffish rakish\n\
rafter raftman raftsman\n\
rag shred tag tatter\n\
ragamuffin tatterdemalion\n\
raggedness roughness\n\
rail railing\n\
rail rails runway\n\
rail train\n\
railroad railway\n\
railroader railwayman trainman\n\
railyard yard\n\
rain rainfall\n\
rain rainwater\n\
raincoat waterproof\n\
rainproof waterproof waterproofed\n\
rainy showery\n\
raisable raiseable\n\
raja rajah\n\
rajpoot rajput\n\
rake slant\n\
ralegh raleigh\n\
rallentando ritardando ritenuto\n\
ramble roam rove stray swan vagabond wander\n\
rambling sprawling straggling straggly\n\
rambotan rambutan\n\
ramee ramie\n\
ramekin ramequin\n\
rameses ramesses ramses\n\
rampant rearing\n\
ranales ranunculales\n\
rand reef witwatersrand\n\
randomised randomized\n\
ranee rani\n\
rangoon yangon\n\
ranitidine zantac\n\
ranking superior\n\
ransomed redeemed\n\
ranter raver\n\
rapacious ravening voracious\n\
raper rapist\n\
raphe rhaphe\n\
rapid speedy\n\
rapier tuck\n\
rare rarefied rarified\n\
rare uncommon\n\
rarity tenuity\n\
rascality shiftiness slipperiness trickiness\n\
rasht resht\n\
rasta rastafarian\n\
ratable rateable\n\
ratafee ratafia\n\
ratan rattan\n\
ratified sanctioned\n\
ratiocinator reasoner\n\
rationality rationalness\n\
ratlin ratline\n\
rattlebrained rattlepated scatterbrained scatty\n\
rattler rattlesnake\n\
ratty shabby tatty\n\
raucous rowdy\n\
raucous strident\n\
raudixin reserpine sandril serpasil\n\
rauvolfia rauwolfia\n\
ravel unravel\n\
raveling ravelling\n\
ravigote ravigotte\n\
razorback rorqual\n\
rb rubidium\n\
re rhenium\n\
reabsorb resorb\n\
reactionary reactionist\n\
reactionary ultraconservative\n\
reactive responsive\n\
reanimated revived\n\
rear rearward\n\
rearward reverse\n\
reasonable sane\n\
reasonable sensible\n\
reasonableness tenability tenableness\n\
rebarbative repellant repellent\n\
rebecca rebekah\n\
reborn revitalised revitalized\n\
rebuker reproacher reprover upbraider\n\
recalcitrance recalcitrancy refractoriness unmanageableness\n\
recap retread\n\
recapture retake\n\
recede retire retreat withdraw\n\
received standard\n\
receiver recipient\n\
recency recentness\n\
recessed sunken\n\
recessionary recessive\n\
recharge reload\n\
recidivist repeater\n\
reciprocative reciprocatory\n\
reclaimable recyclable reusable\n\
reclaimed rescued\n\
recline recumb repose\n\
recluse reclusive withdrawn\n\
recognised recognized\n\
reconstructive rehabilitative\n\
recorder registrar\n\
recourse refuge resort\n\
recoverer rescuer saver\n\
recreant renegade\n\
recriminative recriminatory\n\
rectifiable reparable\n\
rectilineal rectilinear\n\
rectitude uprightness\n\
recuperative restorative\n\
recurring revenant\n\
recurvate recurved\n\
redact redactor reviser rewriter\n\
redberry snakeberry\n\
redbreast robin\n\
redeemable reformable\n\
redeeming redemptive saving\n\
redemptional redemptive redemptory\n\
redfish rosefish\n\
redolent smelling\n\
redstart redtail\n\
reducer reductant\n\
redundance redundancy\n\
redwood sequoia\n\
reedlike reedy\n\
reedy wheezy\n\
reefy shelfy shelvy shoaly\n\
reeking watery\n\
reeler staggerer totterer\n\
ref referee\n\
referee reviewer\n\
reflectance reflectivity\n\
reflection reflectivity reflexion\n\
reflection reflexion\n\
reflectiveness reflectivity\n\
reformative reformatory\n\
refractile refractive\n\
refractiveness refractivity\n\
refractory stubborn\n\
refrigerant refrigerating\n\
refuge safety\n\
regent trustee\n\
regnant reigning ruling\n\
regretful sorry\n\
regulative regulatory\n\
reims rheims\n\
reinforced strengthened\n\
reinforcement strengthener\n\
relation relative\n\
relaxing reposeful restful\n\
release relinquish\n\
release turn\n\
religious spiritual\n\
relocated resettled\n\
remake remaking\n\
remarkable singular\n\
remora suckerfish\n\
renascent resurgent\n\
rend rip rive\n\
rending ripping splitting\n\
renewing restorative revitalising revitalizing reviving\n\
rent rip snag split\n\
renter tenant\n\
renunciant renunciative\n\
reorganised reorganized\n\
rep repp\n\
repair resort\n\
repel repulse\n\
repellant repellent\n\
repellent resistant\n\
repercussion reverberation\n\
repetitious repetitive\n\
replacement successor\n\
replica replication reproduction\n\
replicate retroflex\n\
repository secretary\n\
represser repressor\n\
reptile reptilian\n\
reputability respectability\n\
rescindable voidable\n\
reservation reserve\n\
reserve reticence taciturnity\n\
reservoir source\n\
residual residuary\n\
resilience resiliency\n\
resin rosin\n\
resistance resistor\n\
resistant tolerant\n\
resistless unresisting\n\
resolute unhesitating\n\
resolvable solvable\n\
resolved solved\n\
resonant resonating resounding reverberating reverberative\n\
respectful reverential venerating\n\
respective several various\n\
responsibility responsibleness\n\
restauranter restaurateur\n\
restiveness skittishness\n\
restless uneasy\n\
restless ungratified unsatisfied\n\
restoril temazepam\n\
restrained reticent unemotional\n\
restrictiveness unpermissiveness\n\
resupine supine\n\
retainer servant\n\
retaliative retaliatory retributive retributory vindicatory\n\
retardant retardation retardent\n\
retention retentiveness retentivity\n\
retentiveness retentivity\n\
reticent retiring\n\
reticent untalkative\n\
retick tick\n\
reticular reticulate\n\
retinal retinene\n\
retiring unassuming\n\
retral retrograde\n\
retreat retrograde\n\
retributive retributory vindicatory\n\
retro retroactive\n\
retroflex retroflexed\n\
retrograde retrogressive\n\
retrousse upturned\n\
returning reversive\n\
returning reverting\n\
revealing telling telltale\n\
revengeful vengeful vindictive\n\
revere revers\n\
reverend sublime\n\
reverse verso\n\
revetement revetment\n\
revocable revokable\n\
revolutionary revolutionist subversive subverter\n\
revolutionary rotatory\n\
revolve rotate\n\
revolved rotated\n\
rf rutherfordium unnilquadium unq\n\
rg roentgenium\n\
rh rhodium\n\
rhein rhine\n\
rheinland rhineland\n\
rheologic rheological\n\
rhibhus ribhus\n\
rhino rhinoceros\n\
rhizome rootstalk rootstock\n\
rhizopod rhizopodan\n\
rhodes rodhos\n\
rhodesia zimbabwe\n\
rhombohedral trigonal\n\
rhomboid rhomboidal\n\
rhumba rumba\n\
rhymed rhyming riming\n\
rhymeless rimeless unrhymed unrimed\n\
rhythmic rhythmical\n\
riband ribband\n\
ribavirin virazole\n\
ribbon thread\n\
ribbonlike ribbony\n\
ribonuclease ribonucleinase rnase\n\
ricketiness unsteadiness\n\
rickety shaky wobbly wonky\n\
rickrack ricrac\n\
riddle screen\n\
ride sit\n\
ridge ridgeline\n\
ridge ridgepole rooftree\n\
ridgel ridgeling ridgil ridgling\n\
rifadin rifampin rimactane\n\
riff riffian\n\
rigging tackle\n\
rigid strict\n\
rigidity rigidness\n\
rigorous strict\n\
rigorous stringent\n\
rijstafel rijstaffel rijsttaffel\n\
rile roil\n\
rill rivulet runnel streamlet\n\
ringer toller\n\
ringhals rinkhals\n\
rinse wash\n\
rippled ruffled\n\
ripsaw splitsaw\n\
riskless unhazardous\n\
risky speculative\n\
riverbank riverside\n\
rivet stud\n\
riveter rivetter\n\
road route\n\
roads roadstead\n\
roadside wayside\n\
roadster runabout\n\
roamer rover wanderer\n\
roast roasted\n\
rock stone\n\
rock sway\n\
rocket skyrocket\n\
rockfish striper\n\
roentgen rontgen\n\
rofecoxib vioxx\n\
rolled rolling trilled\n\
roller tumbler\n\
roma rome\n\
roman romanic\n\
romance romanticism\n\
romani romany\n\
romania roumania rumania\n\
romanian roumanian rumanian\n\
romanian rumanian\n\
romanoff romanov\n\
romansh rumansh\n\
romantic romanticist\n\
romantic romanticist romanticistic\n\
romanticist sentimentalist\n\
room way\n\
roomie roommate roomy\n\
rooms suite\n\
roomy spacious\n\
root rootle rout\n\
rootless vagabond\n\
ropedancer ropewalker\n\
ropemaker roper\n\
ropeway tram tramway\n\
ropey ropy\n\
ropey ropy stringy thready\n\
rosaceous rose roseate\n\
rose rosebush\n\
rose rosiness\n\
roselle rozelle sorrel\n\
rosiness ruddiness\n\
rostrum snout\n\
rotate splay\n\
rottenstone tripoli\n\
rouble ruble\n\
rousing stirring\n\
rover scouter\n\
ru ruthenium\n\
ruanda rwanda\n\
ruandan rwandan\n\
rubberlike rubbery\n\
rubberneck rubbernecker\n\
rubbish scrap trash\n\
rubbishy trashy\n\
rudderpost rudderstock\n\
rudimentary vestigial\n\
rugelach ruggelach rugulah\n\
ruined sunk undone\n\
rule ruler\n\
ruler swayer\n\
rundle rung spoke\n\
rung stave\n\
rupestral rupicolous\n\
ruralism rurality\n\
rush rushed\n\
rushlike sedgelike\n\
rusk zwieback\n\
russia ussr\n\
rust rusty\n\
rustproof rustproofed\n\
rutabaga swede\n\
rutted rutty\n\
sabaton solleret\n\
sabayon zabaglione\n\
sabbatic sabbatical\n\
saber sabre\n\
sac sauk\n\
sac theca\n\
sacagawea sacajawea\n\
saccharose sucrose\n\
sacculate sacculated\n\
saccule sacculus\n\
sachem sagamore\n\
sachsen saxe saxony\n\
sacristan sexton\n\
sacristy vestry\n\
saddhu sadhu\n\
saddle saddleback\n\
saddlery tack\n\
safaqis sfax\n\
saffranine safranin safranine\n\
sage salvia\n\
sagittate sagittiform\n\
saguaro sahuaro\n\
sahaptin sahaptino shahaptian\n\
saida sayda sidon\n\
sailplane soar\n\
sake saki\n\
sakkara saqqara saqqarah\n\
sakti shakti\n\
salability salableness\n\
salable saleable\n\
salade sallet\n\
salal shallon\n\
saleroom salesroom showroom\n\
salesgirl saleslady saleswoman\n\
saliva spit spittle\n\
sallow sickly\n\
salmonberry thimbleberry\n\
salonica salonika thessalonica thessaloniki\n\
saloon sedan\n\
salp salpa\n\
salubriousness salubrity\n\
salvadoran salvadorean\n\
salvadoran salvadorean salvadorian\n\
salvage scavenge\n\
salvager salvor\n\
salwar shalwar\n\
samarang semarang\n\
samarcand samarkand\n\
samarium sm\n\
sambar sambur\n\
samiel simoom simoon\n\
samisen shamisen\n\
samoyed samoyede\n\
sampler taster\n\
sana sanaa\n\
sanatarium sanatorium sanitarium\n\
sanctimoniousness sanctimony\n\
sanctionative sanctioning\n\
sand sandpaper\n\
sandaled sandalled\n\
sandarac sandarach\n\
sandbag stun\n\
sandbox sandpile sandpit\n\
sandbur sandspur\n\
sander smoother\n\
sangaree sangria\n\
sanguine sanguineous\n\
sanicle snakeroot\n\
sanitised sanitized\n\
sannyasi sannyasin sanyasi\n\
sapidity sapidness\n\
sapodilla sapota\n\
saponaceous soapy\n\
saprophagous saprozoic\n\
saragossa zaragoza\n\
sarape serape\n\
sarcenet sarsenet\n\
sarcocystidean sarcocystieian sarcosporidian\n\
sarcodine sarcodinian\n\
sarcolemmic sarcolemnous\n\
sard sardine sardius\n\
sardegna sardinia\n\
sardonic snarky\n\
saree sari\n\
sarpanitu zarpanit zirbanit\n\
sartor seamster tailor\n\
sashay sidle\n\
sassaby topi\n\
satiable satisfiable\n\
satiate satiated\n\
satinet satinette\n\
satiny silken silklike silky sleek\n\
satiric satirical\n\
satureia satureja\n\
satyric satyrical\n\
saunter stroll\n\
saute sauteed\n\
sauterne sauternes\n\
savageness savagery\n\
savanna savannah\n\
savory savoury\n\
sawbones surgeon\n\
sax saxophone\n\
saxatile saxicoline saxicolous\n\
saxist saxophonist\n\
sc scandium\n\
scabiosa scabious\n\
scaffolding staging\n\
scag skag smack thunder\n\
scalawag scallywag\n\
scale surmount\n\
scaled scaley scaly\n\
scallop scollop\n\
scallopine scallopini\n\
scamper scurry scuttle skitter\n\
scandalmongering sensationalistic\n\
scantling stud\n\
scanty skimpy\n\
scapular scapulary\n\
scarab scarabaeus\n\
scarabaean scarabaeid\n\
scaramouch scaramouche\n\
scarceness scarcity\n\
scarecrow scarer strawman\n\
scaremonger stirrer\n\
scarfpin tiepin\n\
scarlet vermilion\n\
scathing vituperative\n\
scattergood spender spendthrift\n\
scattergun shotgun\n\
scattering sprinkle sprinkling\n\
scattering sprinkling\n\
scauper scorper\n\
scend surge\n\
scene scenery\n\
scene setting\n\
scene view\n\
sceneshifter shifter\n\
schismatic schismatical\n\
schizoid schizophrenic\n\
schlemiel shlemiel\n\
schlep schlepper shlep shlepper\n\
schlep shlep\n\
schlesien silesia slask slezsko\n\
schlimazel shlimazel\n\
schlockmeister shlockmeister\n\
schmaltz schmalz shmaltz\n\
schmo schmuck shmo shmuck\n\
schnapps schnaps\n\
schnook shnook\n\
schnorrer shnorrer\n\
schoenberg schonberg\n\
school schoolhouse\n\
schrod scrod\n\
schtick schtik shtick shtik\n\
schtickl schtikl shtickl shtikl\n\
schweiz suisse svizzera switzerland\n\
sciara sciarid\n\
scilla squill\n\
scincid skink\n\
sclerosed sclerotic\n\
scomberesocidae scombresocidae\n\
scomberesox scombresox\n\
scoop scoopful\n\
scooter scoter\n\
scope telescope\n\
score scotch\n\
score seduce\n\
scorekeeper scorer\n\
scorner sneerer\n\
scorpio scorpion\n\
scorpio scorpius\n\
scot scotchman scotsman\n\
scotch scots scottish\n\
scotchwoman scotswoman\n\
scoundrel villain\n\
scour scrub\n\
scourge terror threat\n\
scrabbly scrubby\n\
scratchy spotty uneven\n\
scrawler scribbler\n\
scrawniness scrubbiness\n\
scrawniness skinniness\n\
scrawny scrubby stunted\n\
screaky screechy squeaking squeaky squealing\n\
scree talus\n\
screen sieve\n\
screwbean tornillo\n\
scribe scriber\n\
scripted written\n\
scruffy seedy\n\
scrutiniser scrutinizer\n\
scuffle shamble shuffle\n\
scuffle tussle\n\
sculpt sculpture\n\
se selenium\n\
seabird seafowl\n\
seaboard seaside\n\
seaborgium sg\n\
seahorse walrus\n\
seal sealskin\n\
seal varnish\n\
sealant sealer\n\
seamanlike seamanly\n\
seamless unlined unseamed\n\
seamy seedy sleazy sordid squalid\n\
sear sere shriveled shrivelled withered\n\
search seek\n\
searching trenchant\n\
seascape waterscape\n\
seasnail snailfish\n\
seasonable timely\n\
seasonableness timeliness\n\
seasoned veteran\n\
seat sit\n\
seated sitting\n\
seating seats\n\
sebastopol sevastopol\n\
sec unsweet\n\
secobarbital seconal\n\
secondhand used\n\
secrecy secretiveness silence\n\
sectarian sectarist sectary\n\
section segment\n\
sectional sectioned\n\
secular temporal worldly\n\
sedate sober solemn\n\
sedate staid\n\
sedateness solemness solemnity staidness\n\
sedulity sedulousness\n\
seeable visible\n\
seed sow\n\
seeded sown\n\
seedman seedsman\n\
seeland sjaelland zealand\n\
seesaw teeter teeterboard teetertotter\n\
seesaw teeter totter\n\
seesaw teetertotter\n\
segregated unintegrated\n\
segregationist segregator\n\
seigneur seignior\n\
seismal seismic\n\
seismologic seismological\n\
seizer shanghaier\n\
selcraig selkirk\n\
selsyn synchro\n\
selvage selvedge\n\
semanticist semiotician\n\
semestral semestrial\n\
semi semitrailer\n\
semiaquatic subaquatic\n\
semicentenary semicentennial\n\
semiconducting semiconductive\n\
seminarian seminarist\n\
semiotic semiotical\n\
semipro semiprofessional\n\
semisoft softish\n\
semisynthetic synthetic\n\
semite semitic\n\
semitransparency translucence translucency\n\
semitransparent translucent\n\
semitropic semitropical subtropic subtropical\n\
semitropics subtropics\n\
send ship transport\n\
sender transmitter\n\
sensational sensory\n\
sensible sensitive\n\
sensitiser sensitizer\n\
sensitising sensitizing\n\
sensitive sore\n\
sensitiveness sensitivity\n\
sensorial sensory\n\
sensual sultry\n\
sepaline sepaloid\n\
separated spaced\n\
separationist separatist\n\
septal septate\n\
septuple sevenfold\n\
sepulcher sepulchre sepulture\n\
sequenator sequencer\n\
sequence succession successiveness\n\
sequoya sequoyah\n\
seraphic seraphical\n\
serb serbian\n\
serbia srbija\n\
serdica sofia\n\
sergeant serjeant\n\
sericterium serictery\n\
serigraph silkscreen\n\
seriocomic seriocomical\n\
serious sober unplayful\n\
serologic serological\n\
serpentine snakelike snaky\n\
sertraline zoloft\n\
serve service\n\
server waiter\n\
serviceability serviceableness usability usableness useableness\n\
servo servomechanical\n\
servo servomechanism servosystem\n\
sessile stalkless\n\
setline spiller trawl trotline\n\
settee settle\n\
settle subside\n\
settlor trustor\n\
seven vii\n\
sevilla seville\n\
sew stitch\n\
sewage sewerage\n\
sewed sewn stitched\n\
sewing stitchery\n\
sextuple sixfold\n\
shack trail\n\
shade tad\n\
shade tincture tint tone\n\
shades sunglasses\n\
shadow shadower\n\
shadowed shadowy shady umbrageous\n\
shadowy vague wispy\n\
shadowy wraithlike\n\
shagbark shellbark\n\
shagged shaggy\n\
shaitan shaytan\n\
shakable shakeable\n\
shakespeare shakspere\n\
shakespearean shakespearian\n\
shaky shivering trembling\n\
shallow shoal\n\
shallowness superficiality\n\
shamanist shamanistic\n\
shamefaced sheepish\n\
shameless unblushing\n\
shandy shandygaff\n\
shank stem\n\
shank waist\n\
shareholder shareowner stockholder\n\
sharpie sharpy\n\
shattered tattered\n\
shatterproof splinterless splinterproof\n\
shaved shaven\n\
sheared shorn\n\
shed spill\n\
shed throw\n\
shedder spiller\n\
sheepherder sheepman shepherd\n\
sheepish sheeplike\n\
sheeprun sheepwalk\n\
sheik sheikh\n\
sheika sheikha\n\
sheikdom sheikhdom\n\
shellac shellack\n\
shellflower snakehead turtlehead\n\
shelterbelt windbreak\n\
sherbert sherbet\n\
shetland zetland\n\
shifting shifty\n\
shifting unfirm\n\
shiksa shikse\n\
shillalah shillelagh\n\
shimmy wobble\n\
shin shinbone tibia\n\
shinto shintoist shintoistic\n\
shipbuilder shipwright\n\
shipway slipway ways\n\
shipworm teredinid\n\
shirker slacker\n\
shirtwaist shirtwaister\n\
shirty snorty\n\
shitless witless\n\
shittim shittimwood\n\
shiva siva\n\
shiver shudder thrill throb\n\
shlep traipse\n\
shod shodden shoed\n\
shoddiness trashiness\n\
shoddy sordid\n\
shoe skid\n\
shoebill shoebird\n\
shoelace shoestring\n\
shofar shophar\n\
shooter taw\n\
shop store\n\
shop workshop\n\
shopfront storefront\n\
shopkeeper storekeeper tradesman\n\
shopsoiled shopworn\n\
shore shoring\n\
shortened telescoped\n\
shortness truncation\n\
shorts trunks\n\
shoshone shoshoni\n\
shouted yelled\n\
shove stuff\n\
shovel shovelful spadeful\n\
shoveler shoveller\n\
show usher\n\
showcase vitrine\n\
shrew shrewmouse\n\
shrew termagant\n\
shrill strident\n\
shrillness stridence stridency\n\
shriveled shrivelled shrunken\n\
shriveled shrivelled shrunken withered wizen wizened\n\
shudra sudra\n\
shumac sumac sumach\n\
si silicon\n\
siam thailand\n\
siamese tai thai\n\
sib sibling\n\
sichuan szechuan szechwan\n\
sicilia sicily\n\
siderophilin transferrin\n\
sidetrack siding turnout\n\
sieve sift\n\
sieve sift strain\n\
sigmoid sigmoidal\n\
sign signboard\n\
signaler signaller\n\
signatory signer\n\
signature touch\n\
signior signor\n\
sildenafil viagra\n\
siliceous silicious\n\
siliqua silique\n\
silkiness sleekness\n\
sillabub syllabub\n\
silly slaphappy\n\
silva sylva\n\
silvan sylvan\n\
silvanus sylvanus\n\
silverberry silverbush\n\
silvern silvery\n\
silverside silversides\n\
silversmith silverworker\n\
simmpleness simplicity\n\
simonise simonize\n\
simpleness simplicity\n\
simultaneity simultaneousness\n\
simvastatin zocor\n\
sin sinfulness wickedness\n\
sincerity unassumingness\n\
sinew tendon\n\
sinewy tendinous\n\
sinful unholy wicked\n\
singer vocaliser vocalist vocalizer\n\
singhalese sinhala sinhalese\n\
singhalese sinhalese\n\
single unmarried\n\
singleness straightforwardness\n\
singlet undershirt vest\n\
singular unique\n\
singularity uniqueness\n\
sinistrorsal sinistrorse\n\
sinkiang xinjiang\n\
sinoper sinopia sinopis\n\
sinuate sinuous wiggly\n\
sinuosity sinuousness\n\
sion zion\n\
siouan sioux\n\
siphon syphon\n\
siracusa syracuse\n\
sirup syrup\n\
sis sister\n\
sisham sissoo sissu\n\
sisterlike sisterly sororal\n\
site situation\n\
sitsang thibet tibet xizang\n\
six vi\n\
sixpenny threepenny tuppeny twopenny\n\
size sizing\n\
skagerak skagerrak\n\
skeleton underframe\n\
sketch study\n\
sketchy unelaborated\n\
skew skewed\n\
skewer spit\n\
skid slew slide slue\n\
skidder slider slipper\n\
skim skimmed\n\
skim skip skitter\n\
skinny tightfitting\n\
skivvy slavey\n\
skopje skoplje uskub\n\
skunkbush squawbush\n\
sky toss\n\
slack slackness\n\
slapper spanker\n\
slat spline\n\
slate slating\n\
slater woodlouse\n\
slatey slaty\n\
slattern slut trollop\n\
slatternliness sluttishness\n\
slave striver\n\
slaveholder slaver\n\
slavic slavonic\n\
slavish submissive subservient\n\
sled sledge sleigh\n\
sled sleigh\n\
sledge sledgehammer\n\
sleeper slumberer\n\
sleepwalk somnambulate\n\
sleepy sleepyheaded\n\
slender slight slim svelte\n\
slender slim\n\
slender thin\n\
slenderness slightness slimness\n\
slenderness tenuity thinness\n\
sleuth sleuthhound\n\
slew slue swerve trend veer\n\
slice slit\n\
slickness slipperiness\n\
slide slither\n\
slimed slimy\n\
sling slingback\n\
slippery slippy\n\
slippery tricky\n\
slipping slithering\n\
sliver splinter\n\
slivery splintery\n\
slog slug swig\n\
slogger slugger\n\
slop slosh splash splosh squelch squish\n\
slop spill splatter\n\
slosh slush\n\
sloth slothfulness\n\
slouch slump\n\
slovenia slovenija\n\
slow sluggish\n\
slowgoing unenergetic\n\
slug sluggard\n\
sluggish sulky\n\
slumberous slumbery slumbrous somnolent\n\
slumberous slumbrous\n\
smack thwack\n\
smelter smeltery\n\
smitten stricken struck\n\
smokestack stack\n\
smoldering smouldering\n\
smooch spoon\n\
smoothbore unrifled\n\
smoothed smoothened\n\
smother surround\n\
smothered stifled strangled suppressed\n\
smotherer stifler\n\
smothering suffocating suffocative\n\
sn tin\n\
sneaking unavowed\n\
sneaky underhand underhanded\n\
sneering snide supercilious\n\
sniffler sniveler\n\
sniffly snuffling snuffly\n\
snip snippet snipping\n\
snips tinsnips\n\
snobbery snobbishness snobbism\n\
snoop snooper\n\
snow snowfall\n\
snowberry waxberry\n\
snowbird snowflake\n\
snowplough snowplow\n\
soaprock soapstone steatite\n\
soar surge zoom\n\
sociability sociableness\n\
social societal\n\
socialised socialized\n\
socialiser socializer\n\
socialist socialistic\n\
sociobiologic sociobiological\n\
sodden soppy\n\
softie softy\n\
soigne soignee\n\
soil territory\n\
soiled unclean\n\
soja soy soya soybean\n\
solarisation solarization\n\
solarium sunporch sunroom\n\
solidity solidness\n\
solidness substantiality substantialness\n\
solon statesman\n\
solubility solvability\n\
somali somalian\n\
somatogenetic somatogenic\n\
somatotrophin somatotropin sth\n\
sonant voiced\n\
songbird songster\n\
songster songwriter\n\
sonic transonic\n\
soochong souchong\n\
sop sops\n\
soph sophomore\n\
sophistic sophistical\n\
sophonias zephaniah\n\
soprano treble\n\
sordino sourdine\n\
sorgho sorgo\n\
soudan sudan\n\
soundness wisdom wiseness\n\
sourwood titi\n\
sousaphone tuba\n\
souslik suslik\n\
sousse susa susah\n\
southbound southward\n\
southeast southeasterly\n\
southeast southeasterly southeastern\n\
souther southerly\n\
southerly southern\n\
southernmost southmost\n\
southwest southwesterly\n\
southwest southwesterly southwestern\n\
souvlaki souvlakia\n\
sovereign supreme\n\
soy soya soybean\n\
soy soybean\n\
spaceship starship\n\
spacey spacy\n\
spacial spatial\n\
spaciotemporal spatiotemporal\n\
spacious wide\n\
spall spawl\n\
spandrel spandril\n\
spanner wrench\n\
spareness sparseness sparsity thinness\n\
spark sparkle twinkle\n\
sparse thin\n\
speakable utterable\n\
speaker talker utterer verbaliser verbalizer\n\
spearhead spearpoint\n\
specialised specialized\n\
specialiser specialist specializer\n\
specious spurious\n\
speckle stipple\n\
spectrogram spectrograph\n\
spectroscopic spectroscopical\n\
speedwell veronica\n\
sperm spermatozoan spermatozoon\n\
spermatic spermous\n\
spermatocide spermicide\n\
spewer vomiter\n\
sphaerocarpos sphaerocarpus\n\
spic spick spik\n\
spice spicery spiciness\n\
spicule spiculum\n\
spiegel spiegeleisen\n\
spigot tap\n\
spike spindle\n\
spill spillway wasteweir\n\
spindlelegs spindleshanks\n\
spindrift spoondrift\n\
spineless thornless\n\
spinnable spinnbar\n\
spinner spinster\n\
spinous spiny\n\
spiraea spirea\n\
spire steeple\n\
spirilla spirillum\n\
spiritise spiritize\n\
spiritous spirituous\n\
spiritual unearthly\n\
spiritualist spiritualistic\n\
spirochaete spirochete\n\
spit tongue\n\
spitsbergen spitzbergen\n\
splanchnic visceral\n\
splash splosh sprinkle\n\
splashboard washboard\n\
splayfoot splayfooted\n\
splice splicing\n\
spoiled spoilt\n\
spongefly spongillafly\n\
spongelike spongy\n\
spongelike spongy squashy squishy\n\
spontaneity spontaneousness\n\
spontaneous unwritten\n\
spoon spoonful\n\
sporophyl sporophyll\n\
sport sportsman sportswoman\n\
sport summercater\n\
sporting sportsmanlike sporty\n\
sprawl sprawling\n\
sprawl straggle\n\
spray spraying\n\
sprigger stemmer stripper\n\
springbok springbuck\n\
squab squabby\n\
squalling squally\n\
squandered wasted\n\
squat underslung\n\
squatness stubbiness\n\
squelch squelcher\n\
squinched squinting\n\
squirm worm wrestle wriggle writhe\n\
squirmer wiggler wriggler\n\
sr steradian\n\
sr strontium\n\
stabbing wounding\n\
stabilise stabilize\n\
stabilised stabilized\n\
stabiliser stabilizer\n\
stabilising stabilizing\n\
stability stableness\n\
stable stalls\n\
stable static unchanging\n\
stage stagecoach\n\
stagey stagy\n\
staginess theatricality\n\
stagira stagirus\n\
stained varnished\n\
stainless unstained unsullied untainted untarnished\n\
stair step\n\
staircase stairway\n\
stairs steps\n\
stalingrad tsaritsyn volgograd\n\
stalk stem\n\
stall stand\n\
stalwart stout\n\
stalwart stouthearted\n\
stalwartness stoutness\n\
stamina toughness\n\
stammerer stutterer\n\
stamper stomper tramper trampler\n\
standardised standardized\n\
standardiser standardizer\n\
standby understudy\n\
stannic stannous\n\
stapes stirrup\n\
staph staphylococci staphylococcus\n\
staplegun tacker\n\
starkey starr\n\
starved starving\n\
starwort stitchwort\n\
stately statuesque\n\
statesmanlike statesmanly\n\
staunch steadfast unswerving\n\
staunchness steadfastness\n\
staurikosaur staurikosaurus\n\
steadfast unbendable unfaltering unshakable unwavering\n\
stealer thief\n\
steam steamer\n\
steamer steamship\n\
steaming steamy\n\
steelmaker steelman steelworker\n\
steenbok steinbok\n\
stegosaur stegosaurus\n\
stela stele\n\
stenographic stenographical\n\
stenosed stenotic\n\
step tread\n\
stercobilinogen urobilinogen\n\
stereo stereophonic\n\
stereo stereophony\n\
stereotyped stereotypic stereotypical unimaginative\n\
sterile unimaginative uninspired uninventive\n\
sterilised sterilized\n\
sternness strictness\n\
sternutative sternutatory\n\
sternutator sternutatory\n\
stifling sulfurous sulphurous sultry\n\
stigmatic stigmatist\n\
stimulant stimulating\n\
stingy ungenerous\n\
stockinet stockinette\n\
stodginess stuffiness\n\
stodgy stuffy\n\
stoep stoop\n\
stogie stogy\n\
stoic stoical\n\
stoicism stolidity stolidness\n\
stomatal stomatous\n\
stomp stump\n\
stoop stoup\n\
stopcock turncock\n\
stopper stopple\n\
storeroom stowage\n\
storeyed storied\n\
stormy tempestuous\n\
strad stradavarius\n\
stradivari stradivarius\n\
straggler strayer\n\
straighten unbend\n\
straightjacket straitjacket\n\
strain tense\n\
strake wale\n\
strange unknown\n\
strange unusual\n\
strangeness unfamiliarity\n\
stranger unknown\n\
strangle strangulate throttle\n\
strasbourg strassburg\n\
strategian strategist\n\
strategic strategical\n\
stravinskian stravinskyan\n\
straw strew\n\
straw wheat\n\
streaked streaky\n\
stream watercourse\n\
streetcar tram tramcar trolley\n\
strep streptococcal streptococcic\n\
strep streptococci streptococcus\n\
strepsiceros tragelaphus\n\
stressful trying\n\
stretch stretchability stretchiness\n\
stretchable stretchy\n\
stria striation\n\
strickle strike\n\
strictness stringency\n\
string thread\n\
string twine\n\
stringy wiry\n\
striped stripy\n\
strobe stroboscope\n\
stud studhorse\n\
stuffer taxidermist\n\
stumble trip\n\
stumbler tripper\n\
stupa tope\n\
stupid unintelligent\n\
style stylus\n\
styleless unstylish\n\
styracosaur styracosaurus\n\
subaquatic subaqueous submerged submersed underwater\n\
subdivision subsection\n\
subduable subjugable\n\
subjectiveness subjectivity\n\
sublimated sublimed\n\
sublime supreme\n\
sublunar sublunary terrestrial\n\
submarine undersea\n\
submerge submerse\n\
submerged submersed underwater\n\
submergible submersible\n\
subocean suboceanic\n\
subocular suborbital\n\
subordinate subsidiary underling\n\
subordinating subordinative\n\
subshrub suffrutex\n\
subsidised subsidized\n\
subsidiser subsidizer\n\
subsister survivor\n\
subsoil undersoil\n\
substrate substratum\n\
subterranean subterraneous\n\
subterranean subterraneous ulterior\n\
suburb suburbia\n\
suburbanised suburbanized\n\
subversiveness traitorousness treason\n\
subvocaliser subvocalizer\n\
subway underpass\n\
succorer succourer\n\
succuba succubus\n\
sudatorium sudatory\n\
sudatory sudorific\n\
suer suitor wooer\n\
suffrage vote\n\
sugared sweetened\n\
suitability suitableness\n\
suitable suited\n\
sulamyd sulfacetamide\n\
sulfa sulfonamide sulpha\n\
sulfamethazine sulfamezathine\n\
sulfate sulphate\n\
sulfide sulphide\n\
sulfur sulphur\n\
sulfurette sulphurette\n\
sulfuretted sulfurized sulphuretted\n\
sulfuric sulphuric\n\
sulfurous sulphurous\n\
summational summative\n\
sun sunlight sunshine\n\
sunbeam sunray\n\
sunberry wonderberry\n\
sunblock sunscreen\n\
sunburned sunburnt\n\
sunlit sunstruck\n\
sunni sunnite\n\
sunshine temperateness\n\
sup swallow\n\
super superintendent\n\
superficial trivial\n\
superimpose superpose\n\
superior superordinate\n\
superior superscript\n\
superior victor\n\
superlunar superlunary translunar translunary\n\
supermarketeer supermarketer\n\
supermex trevino\n\
supernaturalism supernaturalness\n\
supernaturalist supernaturalistic\n\
supernormal supranormal\n\
superslasher utahraptor\n\
supersonic ultrasonic\n\
superstrate superstratum\n\
supplanter usurper\n\
supplement supplementation\n\
supplemental supplementary\n\
suppliant supplicant supplicatory\n\
support sustain\n\
suppresser suppressor\n\
supraocular supraorbital\n\
sur tyre\n\
surd unvoiced voiceless\n\
sure trusted\n\
surf surfboard\n\
surfactant wetter\n\
surfboarder surfer\n\
surffish surfperch\n\
surge tide\n\
surinam suriname\n\
surly ugly\n\
surmontil trimipramine\n\
surpassing transcendent\n\
surprisingness unexpectedness\n\
surrenderer yielder\n\
suspenseful suspensive\n\
suspicion suspiciousness\n\
sutura suture\n\
svedberg swedenborg\n\
sverige sweden\n\
swab swob\n\
swaddle swathe\n\
swagger swaggie swagman\n\
swaggering swashbuckling\n\
swamp swampland\n\
swanflower swanneck\n\
swank swanky\n\
swathe wrapping\n\
sway swing\n\
sweats sweatsuit\n\
sweeping wholesale\n\
sweetbread sweetbreads\n\
sweetener sweetening\n\
sweetheart sweetie truelove\n\
swell well\n\
swelled vainglorious\n\
sweltering sweltry\n\
swimsuit swimwear\n\
swingletree whiffletree whippletree\n\
swirl twiddle twirl\n\
switch throw\n\
switcher whipper\n\
sybarite voluptuary\n\
syllabicate syllabify syllabise syllabize\n\
syllogiser syllogist syllogizer\n\
sylphic sylphlike\n\
sylvine sylvite\n\
symbolic symbolical\n\
symboliser symbolist symbolizer\n\
symmetric symmetrical\n\
sympathiser sympathizer\n\
symphonic symphonious\n\
symposiarch toastmaster\n\
synaesthetic synesthetic\n\
synagogue tabernacle temple\n\
synchronal synchronic synchronous\n\
synchronised synchronized\n\
synchroniser synchronizer synchronoscope synchroscope\n\
syncretic syncretical syncretistic syncretistical\n\
synecdochic synecdochical\n\
synergetic synergistic\n\
synoecious synoicous\n\
synoptic synoptical\n\
syntactic syntactical\n\
synthesiser synthesist synthesizer\n\
synthesiser synthesizer\n\
synthetic synthetical\n\
syrupy viscous\n\
systematic taxonomic taxonomical\n\
systematist taxonomer taxonomist\n\
ta tantalum\n\
tabbouleh tabooli\n\
tabi tabis\n\
tablespoon tablespoonful\n\
taboo tabu\n\
tabor tabour\n\
taboret tabouret\n\
tach tachometer\n\
tacheometer tachymeter\n\
tacit understood\n\
tact tactfulness\n\
tactile tactual\n\
tactless untactful\n\
tadjik tadzhik tadzhikistan tajik tajikistan\n\
tadzhik tajik\n\
taegu tegu\n\
tailboard tailgate\n\
tailcoat tails\n\
taipeh taipei\n\
taira tayra\n\
takeaway takeout\n\
talc talcum\n\
talentless untalented\n\
tallin tallinn\n\
tallis tallith\n\
tam tammy\n\
tamable tameable\n\
tamal tamale\n\
tamandu tamandua\n\
tamarao tamarau\n\
tamarind tamarindo\n\
tambac tombac tombak\n\
tamburlaine tamerlane timur\n\
tame tamed\n\
tammerfors tampere\n\
tamp tamper\n\
tampion tompion\n\
tan topaz\n\
tangelo ugli\n\
tangible touchable\n\
tangier tangiers\n\
tank tankful\n\
tantaliser tantalizer\n\
tantalising tantalizing\n\
tantalising tantalizing tempting\n\
tantric tantrik\n\
tao taoist\n\
tap tapdance\n\
tapa tappa\n\
tape tapeline\n\
tape taping\n\
taper wick\n\
tapestry tapis\n\
tapper tapster\n\
tapper wiretapper\n\
tarabulus trablous tripoli\n\
tarmac tarmacadam\n\
tarp tarpaulin\n\
tarquin tarquinius\n\
tartar tatar\n\
tartary tatary\n\
tartufe tartuffe\n\
tashkent taskent\n\
tashmit tashmitum\n\
tasse tasset\n\
tasseled tasselled\n\
tatou tatu\n\
tatterdemalion tattered\n\
tautness tightness\n\
taxophytina taxopsida\n\
tb tbit terabit\n\
tb tebibyte terabyte tib\n\
tb terabyte\n\
tb terbium\n\
tbilisi tiflis\n\
tc technetium\n\
tce trichloroethane trichloroethylene\n\
te tellurium\n\
tea teatime\n\
teacup teacupful\n\
teahouse tearoom teashop\n\
teak teakwood\n\
teamster trucker\n\
teased titillated\n\
teasel teasle teazel\n\
teaser tormenter tormentor\n\
teaspoon teaspoonful\n\
tebibit tibit\n\
techie tekki\n\
technical technological\n\
tecumseh tecumtha\n\
tedious verbose windy wordy\n\
tediousness tedium tiresomeness\n\
teepee tepee tipi\n\
teetotaler teetotalist teetotaller\n\
teetotum whirligig\n\
teheran tehran\n\
telegraph telegraphy\n\
telegrapher telegraphist\n\
teleost teleostan\n\
telephoto telephotograph\n\
teleprinter teletypewriter telex\n\
television telly tv\n\
telfer telpher\n\
telferage telpherage\n\
tellurian telluric terrene terrestrial\n\
temp temporary\n\
temper toughness\n\
temporiser temporizer\n\
tendencious tendentious\n\
tendency trend\n\
tenderised tenderized\n\
tenderiser tenderizer\n\
tenderloin undercut\n\
tendrac tenrec\n\
tenebrific tenebrious tenebrous\n\
tennessean volunteer\n\
tennessee tn\n\
tercel tercelet tiercel\n\
teresa theresa\n\
term terminus\n\
ternary treble triple triplex\n\
terrible wicked\n\
terrific terrifying\n\
terry terrycloth\n\
tessin ticino\n\
testate testator\n\
tested tried\n\
tetrachlorethylene tetrachloroethylene\n\
tetrahydrocannabinol thc\n\
tetraiodothyronine thyroxin thyroxine\n\
tetraskele tetraskelion\n\
tevere tiber\n\
texas tx\n\
th thorium\n\
thallium tl\n\
thankless unappreciated ungratifying\n\
thankless ungrateful unthankful\n\
thaw thawing warming\n\
thea theia\n\
theist theistic theistical\n\
thenal thenar\n\
theodolite transit\n\
theologian theologiser theologist theologizer\n\
theoretic theoretical\n\
therapeutic therapeutical\n\
thermodynamic thermodynamical\n\
thermoelectric thermoelectrical\n\
thermograph thermometrograph\n\
thermogravimeter thermohydrometer\n\
thermogravimetric thermohydrometric\n\
thermoregulator thermostat\n\
thermoset thermosetting\n\
thessalia thessaly\n\
thickener thickening\n\
thickhead whistler\n\
thieving thievish\n\
thimble thimbleful\n\
thinned weakened\n\
third tierce\n\
thoriated tittering\n\
thoughtless uncaring unthinking\n\
thoughtlessness unthoughtfulness\n\
thrasher thresher\n\
thread yarn\n\
three trey\n\
threefold treble triple\n\
thriftlessness waste wastefulness\n\
thrip thripid thrips\n\
thrombokinase thromboplastin\n\
thrower throwster\n\
thulium tm\n\
thwartwise transversal transverse\n\
thyreophora thyreophoran\n\
thyroid thyroidal\n\
thyrotrophin thyrotropin tsh\n\
thyrse thyrsus\n\
thysanopter thysanopteron\n\
ti titanium\n\
tianjin tientsin\n\
tibur tivoli\n\
ticker watch\n\
tickling tingling titillating\n\
tidbit titbit\n\
tied trussed\n\
tiglon tigon\n\
timekeeper timer\n\
timidity timorousness\n\
timpanist tympanist\n\
tindal tindale tyndale\n\
tinge undertone\n\
tinker tinkerer\n\
tinkling tinkly\n\
tinner tinsmith\n\
tippytoe tiptoe\n\
tipster tout\n\
tire tyre\n\
tirol tyrol\n\
tirolean tyrolean\n\
tit titmouse\n\
titanosaur titanosaurian\n\
titer titre\n\
titular titulary\n\
tnt trinitrotoluene\n\
toaster wassailer\n\
tocainide tonocard\n\
toe toenail\n\
toetoe toitoi\n\
tolazamide tolinase\n\
tolbooth tollbooth tollhouse\n\
tollbar tollgate\n\
toller tollgatherer tollkeeper tollman\n\
tom tomcat\n\
tomentose tomentous\n\
tonsil tonsilla\n\
toolhouse toolshed\n\
topee topi\n\
topmost upmost uppermost\n\
topographic topographical\n\
topologic topological\n\
topple tumble\n\
tore torus\n\
torino turin\n\
tornado twister\n\
toroid torus\n\
torque torsion\n\
tortuous twisting twisty voluminous winding\n\
toscana tuscany\n\
tosser wanker\n\
totalisator totaliser totalizator totalizer\n\
totaliser totalizer\n\
totalistic totalitarian\n\
tote tug\n\
totipotence totipotency\n\
tottering tottery\n\
toupe toupee\n\
touraco turaco turacou turakoo\n\
touristed touristy\n\
tout touter\n\
tovarich tovarisch\n\
towboat tower tug tugboat\n\
toweling towelling\n\
towline towrope\n\
town township\n\
towner townsman\n\
townie towny\n\
toxicologic toxicological\n\
trabeate trabeated\n\
trabecular trabeculate\n\
traceable trackable\n\
trachea windpipe\n\
trachodon trachodont\n\
traditionalism traditionality\n\
tragic tragical\n\
tragicomic tragicomical\n\
trail train\n\
trainmaster yardmaster\n\
traitor treasonist\n\
tram tramcar\n\
tramline tramway\n\
tramontana tramontane\n\
tramontane transmontane\n\
trample tread\n\
transalpine ultramontane\n\
transcriber translator\n\
transeunt transient\n\
transexual transgendered transsexual\n\
transexual transsexual\n\
transferer transferrer\n\
transgender transgendered\n\
transience transiency transitoriness\n\
transistorised transistorized\n\
transit transportation\n\
transmission transmittance\n\
transmitter vector\n\
transom traverse\n\
transparence transparency\n\
transparence transparency transparentness\n\
transpirate transpire\n\
transudate transudation\n\
transvestic transvestite\n\
travelable traversable\n\
traveled travelled\n\
traveler traveller\n\
treacherous unreliable\n\
treadmill treadwheel\n\
treeless unwooded\n\
treenail trenail trunnel\n\
treillage trellis\n\
trendy voguish\n\
trent trento\n\
trepan trephine\n\
trephritidae trypetidae\n\
triangle triangulum\n\
triangle trigon trilateral\n\
triangular trilateral\n\
tricentenary tricentennial\n\
trichopteran trichopteron\n\
trichromatic trichrome tricolor\n\
tricolor tricolour\n\
tricorn tricorne\n\
tricuspid tricuspidate\n\
tricycle trike velocipede\n\
trifle trivia triviality\n\
trifoliate trifoliated trifoliolate\n\
trigeminal trigeminus\n\
trilobate trilobated trilobed\n\
trinuclear trinucleate trinucleated\n\
trip tripper\n\
tripinnate tripinnated\n\
triskele triskelion\n\
tristan tristram\n\
triumphant victorious\n\
trochlear trochlearis\n\
tropic tropical\n\
trot trotskyist trotskyite\n\
troublemaker troubler\n\
truckle trundle\n\
truculence truculency\n\
truncate truncated\n\
trust trustfulness trustingness\n\
trustful trusting\n\
trustiness trustworthiness\n\
trustworthy trusty\n\
tryptophan tryptophane\n\
tub tubful\n\
tub vat\n\
tube tubing\n\
tubercular tuberculous\n\
tufa tuff\n\
tughrik tugrik\n\
tulestoma tulostoma\n\
tulipwood whitewood\n\
tulostomaceae tulostomataceae\n\
tumbrel tumbril\n\
tuna tunny\n\
tuneless unmelodious untuneful\n\
tungsten wolfram\n\
tunicata urochorda urochordata\n\
tunicate urochord urochordate\n\
tupek tupik\n\
turbidity turbidness\n\
turbinal turbinate\n\
turbulence turbulency\n\
turcoman turkmen turkoman\n\
turkestan turkistan\n\
turkmen turkmenia turkmenistan turkomen\n\
turnout widening\n\
turpentine turps\n\
turtle turtleneck\n\
tussah tusseh tusser tussore tussur\n\
tutsi watusi watutsi\n\
tux tuxedo\n\
twerp twirp twit\n\
twiggy twiglike\n\
twine wind wrap\n\
typographic typographical\n\
tyr tyrr\n\
tyrannosaur tyrannosaurus\n\
tyrocidin tyrocidine\n\
tyrolean tyrolese\n\
u308 yellowcake\n\
udmurt votyak\n\
uighur uigur uygur\n\
uke ukulele\n\
ukraine ukrayina\n\
ulfila ulfilas wulfila\n\
ull ullr\n\
ult ultimo\n\
ultraviolet uv\n\
umbellar umbellate\n\
ump umpire\n\
umpteen umteen\n\
umpteenth umptieth umteenth\n\
unabashed unembarrassed\n\
unacceptability unacceptableness\n\
unacceptable unaccepted\n\
unaccommodating unobliging\n\
unaccountable unexplainable\n\
unaccredited unlicenced unlicensed\n\
unachievable unattainable undoable unrealizable\n\
unadapted unadjusted\n\
unadorned undecorated\n\
unaerated unoxygenated\n\
unaffected unmoved untouched\n\
unaffectionate uncaring\n\
unafraid untroubled\n\
unai unau\n\
unalert unvigilant unwatchful\n\
unaltered unchanged\n\
unambiguity unequivocalness\n\
unambiguous unequivocal univocal\n\
unanalyzable undecomposable\n\
unanimous whole\n\
unannounced unheralded unpredicted\n\
unanswered unreciprocated unrequited\n\
unanticipated unforeseen unseen\n\
unappareled unattired unclad undressed ungarbed ungarmented\n\
unappealing unlikable unlikeable unsympathetic\n\
unappetising unappetizing\n\
unappetisingness unappetizingness\n\
unappreciated unsung unvalued\n\
unapproachable unreachable unreached\n\
unarmored unarmoured\n\
unascertainable undiscoverable\n\
unascribable unattributable\n\
unassailable untouchable\n\
unassisted unbacked\n\
unasterisked unstarred\n\
unattached uncommitted\n\
unattractive untempting\n\
unauthorised unauthorized\n\
unauthorised unauthorized wildcat\n\
unbaffled unconfused\n\
unbaptised unbaptized\n\
unbarred unbolted unlatched unlocked unsecured\n\
unbarreled unbarrelled\n\
unbeaten unconquered unvanquished\n\
unbeknown unbeknownst\n\
unbent unbowed\n\
unbiased unbiassed\n\
unbleached uncolored undyed\n\
unblemished unmarred unmutilated\n\
unblinking unflinching unintimidated unshrinking\n\
unbrace unlace untie\n\
unbranched unbranching\n\
unbridled unchecked uncurbed ungoverned\n\
unbroken unploughed unplowed\n\
unbuttoned unfastened\n\
unbuttoned unlaced\n\
uncategorised uncategorized unsorted\n\
unceremonial unceremonious\n\
uncertain unsealed\n\
unchained unfettered unshackled untied\n\
unchallenged undisputed unquestioned\n\
unchristianly unchristlike\n\
unclimbable unscalable\n\
unclimbable unsurmountable\n\
uncluttered unlittered\n\
uncoerced unforced willing\n\
uncollected ungathered\n\
uncolored uncoloured\n\
uncommercialised uncommercialized\n\
uncompensated unsalaried\n\
uncomplicated unsophisticated\n\
uncomplimentary unflattering\n\
uncompounded unmixed\n\
unconditional unconditioned\n\
unconfined unimprisoned\n\
uncontaminated unpolluted\n\
uncontrived unstudied\n\
uncontrollable uncorrectable unmanageable\n\
uncontrollable unmanageable\n\
unconverted unpersuaded\n\
uncorrected undisciplined\n\
uncorroborated unsubstantiated\n\
uncorrupted undefiled\n\
uncorrupted unspoiled\n\
uncrystallised uncrystallized\n\
uncultivable uncultivatable\n\
uncurved uncurving\n\
uncut unmown\n\
uncut untrimmed\n\
undaunted undismayed unshaken\n\
undecided undetermined unresolved\n\
undefined vague\n\
undependability undependableness unreliability unreliableness\n\
undependable unreliable\n\
undepicted unpictured\n\
underarm underhand underhanded\n\
underbelly underbody\n\
underclothes underclothing underwear\n\
undercoat underfur\n\
undercoat underseal\n\
undercoated undersealed\n\
underfed undernourished\n\
undergarment unmentionable\n\
undergrad undergraduate\n\
underhung undershot underslung\n\
underlay underlayment\n\
undermanned understaffed\n\
undersize undersized\n\
understated unostentatious unpretentious\n\
undeserving unworthy\n\
undesirable unsuitable\n\
undesirable unwanted\n\
undesired unsought\n\
undesiring undesirous\n\
undeterred undiscouraged\n\
undeveloped unexploited\n\
undeviating unswerving\n\
undifferentiated uniform\n\
undiminished unrelieved\n\
undischarged unexploded\n\
undisciplined ungoverned\n\
undisclosed unrevealed\n\
undiscovered unexplored\n\
undistinguished unexceptional\n\
undo unwrap\n\
undone unstuck\n\
undreamed undreamt unimagined\n\
undue unjustified unwarranted\n\
undulant undulatory\n\
undulation wave\n\
uneconomic uneconomical\n\
uneconomical wasteful\n\
unedifying unenlightening\n\
unembellished unornamented\n\
unengaged unpledged unpromised\n\
unenlightening unilluminating\n\
unentitled unqualified\n\
unequalised unequalized\n\
unevenness variability\n\
unexceeded unexcelled unsurpassed\n\
unexceptionable unimpeachable\n\
unexciting unstimulating\n\
unexclusive unrestricted\n\
unexpended unspent\n\
unexpressed unsaid unspoken unstated unuttered unverbalised unverbalized unvoiced\n\
unfailing unflagging\n\
unfair unjust\n\
unfashionable unstylish\n\
unfastened untied\n\
unfavorable unfavourable\n\
unfavorableness unfavourableness\n\
unfertilised unfertilized unimpregnated\n\
unfilmed untaped\n\
unfirm unsteady\n\
unfit unsound\n\
unflurried unflustered unperturbed unruffled\n\
unfocused unfocussed\n\
unforced unstrained\n\
unfulfilled unrealised unrealized\n\
ungentlemanlike ungentlemanly\n\
ungraded unordered unranked\n\
ungreased unlubricated\n\
unguaranteed unsecured\n\
unguiculate unguiculated\n\
unhallowed unholy\n\
unhampered unhindered\n\
unhardened untempered\n\
unharmed unhurt unscathed whole\n\
unheated unwarmed\n\
unhomogenised unhomogenized\n\
unhoped unthought\n\
uniat uniate\n\
uniform unvarying\n\
uniformity uniformness\n\
unindustrialised unindustrialized\n\
uninebriated unintoxicated\n\
uninfluenced unswayed untouched\n\
uninquiring uninquisitive\n\
unintentional unplanned unwitting\n\
uninviting untempting\n\
uniovular uniovulate\n\
unironed wrinkled\n\
unit whole\n\
unitise unitize\n\
universalist universalistic\n\
unkindly unsympathetic\n\
unknot unpick unravel unscramble untangle\n\
unknown unsung\n\
unlabeled unlabelled untagged\n\
unlaced untied\n\
unlamented unmourned\n\
unlaureled unlaurelled\n\
unlawful wrongful\n\
unleavened unraised\n\
unlighted unlit\n\
unlikable unlikeable\n\
unlikelihood unlikeliness\n\
unlivable unliveable\n\
unloose unloosen\n\
unlovely unpicturesque\n\
unmanageable unwieldy\n\
unmanful unmanlike unmanly\n\
unmannered unmannerly\n\
unmarketable unmerchantable unvendible\n\
unmechanised unmechanized\n\
unmelodic unmelodious unmusical\n\
unmingled unmixed\n\
unmodernised unmodernized\n\
unnaturalised unnaturalized\n\
unnecessary unneeded\n\
unneighborly unneighbourly\n\
unnotched untoothed\n\
unobjective unverifiable\n\
unobservant unseeing\n\
unobserved unseen\n\
unoccupied untenanted\n\
unorganised unorganized\n\
unostentatious unpretending unpretentious\n\
unpaid volunteer\n\
unpalatability unpalatableness\n\
unpasteurised unpasteurized\n\
unpeopled unpopulated\n\
unperceived unremarked\n\
unperceiving unperceptive\n\
unpersuadable unsuasible\n\
unpracticed unpractised unversed\n\
unpredictability volatility\n\
unprepossessing unpresentable\n\
unprocessed unrefined\n\
unprofitability unprofitableness\n\
unpronounceable unutterable\n\
unproved unproven\n\
unprovocative unprovoking\n\
unquotable unrepeatable\n\
unreassuring worrisome\n\
unrecognisable unrecognizable\n\
unrecognised unrecognized\n\
unreflective unthinking unthoughtful\n\
unreformable unregenerate\n\
unrefreshed unrested\n\
unregenerate unregenerated\n\
unregretful unregretting\n\
unrenewed unrevived\n\
unresolved unsolved\n\
unrhythmic unrhythmical\n\
unroll unwind\n\
unsalable unsaleable\n\
unsalted unseasoned\n\
unsated unsatiated unsatisfied\n\
unschooled untaught untutored\n\
unseasonable untimely\n\
unseasonableness untimeliness\n\
unseasoned untested untried\n\
unseeded unsown\n\
unserviceable unusable unuseable\n\
unservile unsubmissive\n\
unshaped unshapen\n\
unshaved unshaven\n\
unsheared unshorn\n\
unshod unshoed\n\
unsloped upright\n\
unsociability unsociableness\n\
unsoiled unspotted unstained\n\
unsophisticated unworldly\n\
unsound unstable\n\
unspecialised unspecialized\n\
unspoiled unspoilt\n\
unstained unvarnished\n\
unsterilised unsterilized\n\
unsuspecting unsuspicious\n\
unsymmetric unsymmetrical\n\
unsympathising unsympathizing\n\
untasted untouched\n\
untested untried\n\
untired unwearied unweary\n\
untrammeled untrammelled\n\
untraveled untravelled\n\
untrustiness untrustworthiness\n\
untrustworthy untrusty\n\
ununbium uub\n\
ununhexium uuh\n\
ununpentium uup\n\
ununquadium uuq\n\
ununtrium uut\n\
unvaried unvarying\n\
unvulcanised unvulcanized\n\
unwed unwedded\n\
unwelcome unwished\n\
unwrinkled wrinkleless\n\
up upward\n\
uppishness uppityness\n\
uppsala upsala\n\
upright vertical\n\
upstair upstairs\n\
upwind weather\n\
urania venus\n\
urbanised urbanized\n\
urd urth\n\
usable useable\n\
usbeg usbek uzbak uzbeg uzbek\n\
useful utile\n\
useful utilitarian\n\
usefulness utility\n\
usher ussher\n\
ut utah\n\
uterus womb\n\
utiliser utilizer\n\
utricle utriculus\n\
utterer vocaliser vocalizer\n\
utu utug\n\
uveal uveous\n\
uxorial wifelike wifely\n\
uzbek uzbekistan\n\
va virginia\n\
vacationer vacationist\n\
vaccine vaccinum\n\
vacillant vacillating wavering\n\
vacuity vacuum\n\
vacuolate vacuolated\n\
vale valley\n\
valence valency\n\
valetta valletta\n\
valetudinarian valetudinary\n\
valiant valorous\n\
validity validness\n\
valuable worthful\n\
valvelet valvula valvule\n\
vancocin vancomycin\n\
vandalise vandalize\n\
vane weathervane\n\
vane web\n\
vapor vapour\n\
vaporific vaporish vaporous vapourific vapourish vapourous\n\
vaporiser vaporizer\n\
vaporize zap\n\
variability variableness variance\n\
variable varying\n\
varicolored varicoloured variegated\n\
variolar variolic variolous\n\
various versatile\n\
varment varmint\n\
varmint vermin\n\
vas vessel\n\
vasodilative vasodilator\n\
veal veau\n\
veg vegetable veggie\n\
vegetal vegetational vegetative\n\
vegetal vegetative\n\
vegetative vegetive\n\
veil velum\n\
vein vena\n\
veined veinlike venose\n\
velban vinblastine\n\
veld veldt\n\
velour velours\n\
velvet velvety\n\
velvetleaf velvetweed\n\
veneer veneering\n\
venerability venerableness\n\
venetia veneto\n\
venezia venice\n\
venomous virulent\n\
vent volcano\n\
ventricose ventricous\n\
venula venule\n\
veps vepse vepsian\n\
verbena vervain\n\
verdandi verthandi\n\
vergil virgil\n\
verifier voucher\n\
vermicular vermiculate vermiculated\n\
vermiculate wormy\n\
vermont vt\n\
vernacular vulgar\n\
vernal youthful\n\
verrazano verrazzano\n\
verruca wart\n\
verrucose wartlike warty\n\
verticillate verticillated whorled\n\
vertu virtu\n\
verve vitality\n\
vesicant vesicatory\n\
vessel watercraft\n\
vest waistcoat\n\
vet veteran\n\
vet veterinarian veterinary\n\
vibe vibration\n\
vibes vibraharp vibraphone\n\
vibist vibraphonist\n\
vibrant vivacious\n\
vibrio vibrion\n\
vibrissa whisker\n\
vicereine viceroy\n\
victimiser victimizer\n\
victor winner\n\
victorious winning\n\
victualer victualler\n\
vidar vithar vitharr\n\
vigilance watchfulness\n\
vigilant wakeful\n\
villainousness villainy\n\
vilna vilnius vilno wilno\n\
vinaceous vinous\n\
vinegariness vinegarishness\n\
vinegarish vinegary\n\
vinery vineyard\n\
vino wine\n\
vintner winemaker\n\
viocin viomycin\n\
virgin virgo\n\
viricidal virucidal\n\
viricide virucide\n\
viroid virusoid\n\
virtue virtuousness\n\
virulence virulency\n\
viscometer viscosimeter\n\
viscometric viscosimetric\n\
viscosity viscousness\n\
visibility visibleness\n\
visitant visitor\n\
visor vizor\n\
visualiser visualizer\n\
vitaceae vitidaceae\n\
vitellus yolk\n\
viverridae viverrinae\n\
voltarean voltarian\n\
volumetric volumetrical\n\
voluntary volunteer\n\
voyeuristic voyeuristical\n\
vulcaniser vulcanizer\n\
vulgariser vulgarizer\n\
vulpecular vulpine\n\
vulval vulvar\n\
wa washington\n\
wag waggle\n\
waggle wamble\n\
waggon wagon\n\
waggoner wagoner\n\
waggonwright wagonwright wainwright\n\
wahabi wahhabi\n\
wainscot wainscoting wainscotting\n\
wainscoting wainscotting\n\
waist waistline\n\
wakeful waking\n\
waldmeister woodruff\n\
walker zimmer\n\
wallop whack wham whop\n\
wallow welter\n\
warehouseman warehouser\n\
warmness warmth\n\
warragal warrigal\n\
warsaw warszawa\n\
wartweed wartwort\n\
washy watery\n\
waster wastrel\n\
watcher watchman\n\
waterbird waterfowl\n\
watercolor watercolour\n\
watercolorist watercolourist\n\
watercourse waterway\n\
waver weave\n\
waxen waxlike waxy\n\
waxen waxy\n\
wayland wieland\n\
wb weber\n\
weathered weatherworn\n\
weatherstrip weatherstripping\n\
weaver weaverbird\n\
web www\n\
wed wedded\n\
weight weightiness\n\
weight weighting\n\
weird wyrd\n\
weisenheimer wiseacre wisenheimer\n\
welcher welsher\n\
wellhead wellspring\n\
westbound westerly westward\n\
westerly western\n\
westernmost westmost\n\
whacker whopper\n\
wheaten wholemeal\n\
wheeler wheelwright\n\
whidah whydah\n\
whin whinstone\n\
whip whisk\n\
whipping whipstitch whipstitching\n\
whiskey whisky\n\
whizbang whizzbang\n\
whizz zoom\n\
whoosh woosh\n\
whoremaster whoremonger\n\
wi wisconsin\n\
wiccan witch\n\
wickiup wikiup\n\
wickliffe wiclif wyclif wycliffe\n\
widgeon wigeon\n\
widower widowman\n\
wifi wlan\n\
wiggler wriggler\n\
wiggly wriggling wriggly writhing\n\
wilful willful\n\
wimpish wimpy\n\
winch windlass\n\
wind wreathe\n\
windburned windburnt\n\
window windowpane\n\
windscreen windshield\n\
winey winy\n\
wingspan wingspread\n\
wintery wintry\n\
wireman wirer\n\
wisdom wiseness\n\
wisplike wispy\n\
wistaria wisteria\n\
withe withy\n\
wivern wyvern\n\
wodan woden\n\
woebegone woeful\n\
wolfbane wolfsbane\n\
wolfish wolflike\n\
womanlike womanliness\n\
wood woodwind\n\
woodcreeper woodhewer\n\
woodgrain woodiness\n\
woodiness woodsiness\n\
woodman woodsman\n\
woodman woodsman woodworker\n\
wool woolen woollen\n\
woolen woollen\n\
woolly wooly\n\
workbag workbasket workbox\n\
working workings\n\
workingman workman\n\
workings works\n\
worse worsened\n\
worshiper worshipper\n\
wrack wreck\n\
wrap wrapper\n\
wrap wrapper wrapping\n\
wrathful wroth wrothful\n\
wrench wring\n\
wrinkled wrinkly\n\
wuerzburg wurzburg\n\
wy wyoming\n\
wyat wyatt\n\
xanthous yellowish\n\
xe xenon\n\
xerophile xerophyte\n\
xylene xylol\n\
yachtsman yachtswoman\n\
yank yankee\n\
yarmelke yarmulka yarmulke\n\
yashmac yashmak\n\
yb ybit yottabit\n\
yb yib yobibyte yottabyte\n\
yb yottabyte\n\
yb ytterbium\n\
yeastlike yeasty\n\
yenisei yenisey\n\
ygdrasil yggdrasil\n\
yibit yobibit\n\
yoghourt yoghurt yogurt\n\
yogic yogistic\n\
yon yonder\n\
younker youth\n\
yucatec yucateco\n\
yugoslav yugoslavian\n\
zacharias zechariah\n\
zag zig zigzag\n\
zairean zairese\n\
zapotec zapotecan\n\
zarathustra zoroaster\n\
zb zbit zettabit\n\
zb zebibyte zettabyte zib\n\
zb zettabyte\n\
zebibit zibit\n\
ziggurat zikkurat zikurat\n\
zinc zn\n\
zip zipper\n\
zirconium zr\n\
zombi zombie\n\
zona zone\n\
zonal zonary\n\
zonula zonule\n\
zooflagellate zoomastigote\n\
zu zubird\n\
zygnemales zygnematales\n\
zygomorphic zygomorphous\n\
zygomycota zygomycotina\n\
zymolytic zymotic\n\
";

/// Byte offset of every word slot in the blob, sorted by the word it
/// starts (ties by offset): the binary-search index.
pub(super) static WORDNET_INDEX: &[u32] = &[
    0, 21, 28, 48, 60, 92, 106, 119, 131, 151, 170, 192, 215, 233, 260, 267,
    274, 285, 298, 305, 313, 327, 347, 355, 364, 377, 394, 403, 413, 429, 445, 453,
    462, 478, 492, 499, 507, 522, 537, 545, 554, 569, 587, 596, 606, 623, 641, 651,
    667, 683, 691, 707, 720, 727, 742, 749, 757, 766, 774, 781, 789, 798, 808, 816,
    819, 830, 844, 852, 867, 875, 884, 894, 903, 911, 920, 930, 941, 948, 967, 982,
    1003, 1015, 1022, 1036, 1043, 1051, 1060, 1068, 1075, 1083, 1092, 1102, 1108, 1130, 1141, 1150,
    1164, 1170, 1177, 1185, 1192, 1198, 1205, 1213, 1222, 1229, 1239, 1251, 1274, 1288, 1295, 1303,
    1312, 1320, 1327, 1335, 1344, 1354, 1362, 1372, 1387, 1403, 1411, 1420, 1430, 1439, 1447, 1456,
    1466, 1477, 1486, 1498, 1523, 1538, 1547, 1557, 1568, 1578, 1587, 1597, 1608, 1620, 1627, 1638,
    1657, 1670, 1685, 1692, 1700, 1709, 1717, 1724, 1732, 1741, 1751, 1757, 1767, 1779, 1794, 1803,
    1798, 1821, 1834, 1847, 1853, 1862, 1883, 1915, 1943, 1973, 1980, 1987, 1994, 2002, 2044, 2059,
    2075, 2095, 2113, 2145, 2166, 2181, 1837, 2210, 2240, 2267, 2276, 2285, 2304, 2294, 2332, 2369,
    2384, 2409, 2431, 2439, 2447, 2463, 2512, 2542, 2554, 2587, 2608, 2625, 2632, 2642, 2659, 2675,
    2666, 2649, 2682, 2692, 2726, 2748, 2773, 2786, 2806, 2377, 2828, 2847, 2872, 2907, 2923, 2952,
    3011, 3050, 3069, 3114, 3132, 3080, 3180, 3191, 3205, 3237, 3252, 3269, 3281, 3308, 3325, 3343,
    3365, 3317, 3378, 3394, 3405, 3417, 3350, 3429, 3334, 3467, 3482, 3500, 2157, 3536, 3553, 3569,
    3589, 3608, 3660, 3740, 3777, 3810, 3824, 3843, 3881, 3850, 3912, 3920, 3929, 3953, 3985, 4018,
    4029, 4042, 4092, 4126, 4145, 4197, 4225, 4246, 4235, 3785, 4297, 4328, 4346, 4368, 4357, 4307,
    4380, 4390, 3863, 4441, 4399, 4452, 4463, 4482, 4531, 4620, 3245, 4660, 4694, 4723, 4741, 4772,
    4790, 2548, 4804, 4816, 4810, 4824, 4845, 4873, 4885, 4906, 4894, 4932, 4954, 4966, 4990, 5009,
    5035, 5045, 5063, 5072, 5080, 5090, 5100, 5110, 5122, 5133, 5162, 5170, 5179, 5200, 5226, 5247,
    5276, 5213, 5297, 5315, 5344, 5329, 5375, 5408, 5428, 5418, 5459, 5504, 5520, 5540, 5570, 5621,
    5466, 5530, 5645, 5681, 5740, 5774, 5795, 5818, 5853, 5879, 5910, 5924, 5938, 5964, 5990, 6011,
    6032, 6057, 6082, 6069, 6176, 6199, 6254, 6276, 6317, 6351, 6404, 6428, 6448, 6492, 6526, 5655,
    6537, 5668, 6547, 6589, 6609, 6625, 6639, 6597, 6669, 6693, 6724, 6739, 6762, 6771, 6788, 6828,
    6799, 6810, 6819, 6849, 6886, 6903, 6934, 6953, 6975, 7000, 7091, 7006, 7097, 7116, 7174, 6983,
    7192, 7223, 7244, 7265, 7319, 7330, 7358, 7376, 7394, 7436, 7444, 7410, 7472, 7487, 7511, 7519,
    7527, 7538, 6214, 7557, 7591, 7603, 7620, 7695, 7714, 7728, 7769, 7778, 7788, 7598, 6991, 7014,
    7812, 7844, 7817, 7882, 7899, 7909, 7920, 7932, 7824, 7834, 7945, 7954, 7961, 7968, 7976, 7984,
    7995, 8012, 8043, 8055, 8067, 8081, 8090, 8101, 8121, 8142, 5780, 7019, 8160, 8174, 8195, 8204,
    8214, 8241, 7125, 8260, 8289, 8299, 8310, 8328, 8338, 8349, 8369, 8398, 8407, 8482, 8503, 8511,
    8492, 8520, 8532, 4876, 8541, 8558, 8571, 8586, 8600, 8615, 8629, 8550, 8644, 8663, 8684, 8702,
    8723, 8736, 8751, 8767, 8788, 8808, 8830, 8850, 8839, 8799, 8868, 8886, 8917, 8933, 8948, 8971,
    8713, 8992, 9016, 9031, 9040, 8999, 9050, 9128, 9142, 9160, 9183, 9198, 9223, 9256, 9299, 9264,
    9315, 9328, 9352, 9372, 9388, 9404, 9363, 9396, 9433, 9449, 9457, 9465, 9495, 9518, 9536, 9552,
    9603, 9647, 9565, 9672, 9681, 9691, 9701, 9725, 9776, 9798, 9819, 9835, 9850, 9785, 9878, 9920,
    9946, 9963, 9990, 10027, 10000, 10009, 10045, 10077, 10057, 10118, 10154, 10199, 10213, 10224, 10234, 4777,
    10255, 10281, 10294, 5691, 10307, 10325, 10360, 10369, 10378, 5699, 10392, 10420, 10444, 10457, 10485, 10500,
    5476, 10544, 5486, 10555, 10566, 10589, 10617, 10629, 10662, 10711, 10725, 10741, 10776, 10811, 10844, 10852,
    10861, 10886, 10873, 10912, 10493, 10948, 10968, 10987, 11005, 11025, 11035, 11069, 11096, 8412, 11117, 8433,
    11137, 11152, 11163, 11183, 11250, 11259, 11270, 11281, 11291, 11301, 11312, 11331, 11351, 11365, 11384, 11408,
    11431, 11442, 11395, 11419, 11462, 11483, 11540, 11560, 11600, 11640, 11678, 11695, 11714, 11686, 11731, 11758,
    11772, 11795, 11813, 11830, 11848, 11882, 11967, 11991, 12011, 12031, 12045, 12057, 12089, 12136, 12159, 12176,
    12206, 10398, 10409, 12224, 12254, 12262, 12281, 12327, 12365, 12336, 12411, 12451, 12470, 12474, 12479, 12486,
    12495, 12509, 12532, 12544, 12557, 12577, 12591, 12584, 12606, 12629, 12638, 12710, 12725, 12741, 12776, 12791,
    12748, 12813, 12830, 12861, 12894, 12932, 12957, 12980, 13016, 13027, 12838, 12849, 13040, 13059, 13078, 13100,
    13089, 13112, 12757, 12797, 13124, 13144, 13176, 13193, 13235, 13263, 13203, 13282, 13307, 13324, 13335, 13370,
    13393, 13400, 13408, 13473, 13419, 13504, 13535, 13554, 13582, 13612, 13639, 13594, 13683, 13713, 13721, 13728,
    13750, 13774, 13829, 13762, 13852, 13898, 13922, 13930, 13939, 13958, 13977, 14001, 14037, 14075, 14091, 14119,
    14098, 14106, 14126, 2699, 2705, 2733, 2712, 14139, 14154, 14176, 11190, 14196, 14239, 14256, 14271, 14286,
    14301, 14330, 14343, 14363, 14353, 14374, 14389, 14413, 14428, 14435, 14448, 14482, 14495, 14509, 14520, 14546,
    14559, 14579, 12646, 14602, 14622, 14642, 14656, 14668, 14681, 14705, 14717, 14731, 14759, 14779, 14817, 14838,
    14876, 14894, 14848, 14859, 14923, 14944, 14967, 15005, 15044, 15077, 15090, 15108, 14586, 15160, 15179, 15207,
    15217, 15237, 15255, 15279, 15301, 15314, 15338, 15438, 15468, 15484, 15508, 15491, 15533, 15553, 15542, 15570,
    15574, 15580, 15598, 15673, 15693, 15725, 15703, 15589, 15608, 15744, 15774, 6361, 15787, 15801, 15823, 15848,
    15753, 15864, 15880, 8150, 15919, 15929, 15938, 15951, 15966, 15976, 15988, 15998, 16010, 16027, 16057, 16071,
    16078, 16089, 10387, 16109, 16124, 16139, 16145, 12502, 15307, 15320, 16160, 16167, 15327, 16176, 16187, 16239,
    11197, 16259, 16302, 16330, 16338, 16353, 16373, 16391, 16403, 16416, 12871, 16431, 16442, 12941, 16457, 16512,
    16530, 12822, 16553, 12989, 13069, 12880, 16568, 16578, 16592, 16610, 16634, 16674, 16689, 16561, 16703, 16717,
    12762, 16437, 16745, 16786, 16800, 16825, 16835, 5142, 5153, 7990, 1774, 16847, 16857, 16877, 16935, 5785,
    5790, 16948, 16962, 16978, 16989, 16981, 17011, 17020, 17030, 17040, 17052, 17081, 17100, 17114, 17086, 16828,
    17130, 17136, 17143, 17163, 17171, 17179, 17190, 17200, 17208, 1787, 17217, 17233, 17225, 17251, 17276, 17305,
    17323, 17347, 17365, 17374, 17385, 17397, 17411, 17446, 17475, 17522, 17533, 17549, 17574, 17585, 17596, 17615,
    17632, 17652, 17657, 17670, 17688, 17737, 17752, 17758, 14162, 17767, 17777, 17788, 17804, 17814, 17847, 17852,
    17858, 17870, 17893, 17880, 17909, 17919, 17931, 17939, 17947, 17958, 17971, 17990, 17981, 18000, 18021, 18011,
    18032, 18042, 18050, 18058, 18065, 18073, 18091, 18120, 18140, 18082, 17092, 2719, 18156, 18169, 18190, 18209,
    18239, 18312, 18322, 18247, 18374, 8730, 17676, 18394, 18409, 18420, 18451, 18460, 17314, 17335, 18470, 18478,
    18487, 18497, 18524, 18540, 17356, 18556, 18578, 17485, 7497, 18596, 18623, 18640, 18658, 18668, 18680, 18692,
    18704, 18723, 18711, 18731, 18745, 18758, 18883, 18915, 18941, 18893, 18953, 19007, 19013, 19022, 19035, 19068,
    19084, 19104, 19029, 19131, 19183, 19208, 19218, 19244, 19266, 19286, 19307, 19316, 19327, 19338, 19351, 19363,
    19373, 19395, 19429, 19441, 19448, 19472, 19509, 19535, 19553, 19544, 19586, 19598, 19608, 19618, 19662, 19682,
    19700, 19711, 19758, 19805, 19827, 19878, 19899, 19910, 19923, 19949, 19936, 19962, 19975, 19986, 19999, 20012,
    20027, 20044, 20051, 20066, 20082, 20101, 9380, 20159, 20176, 20198, 20220, 20186, 20208, 20252, 20260, 20277,
    20293, 20308, 20326, 20348, 20372, 20392, 20407, 20441, 20460, 16992, 20470, 17002, 20412, 20419, 20451, 20480,
    20503, 20551, 20567, 20580, 20598, 20630, 20644, 20696, 20655, 20731, 20755, 20776, 20814, 20784, 20842, 20858,
    20883, 20928, 20948, 20970, 20984, 21038, 21060, 21071, 21081, 21091, 21109, 21140, 21123, 21100, 21175, 21199,
    21219, 21245, 21268, 21293, 21254, 21315, 21327, 21353, 21399, 21421, 21439, 21467, 21483, 21503, 21493, 21513,
    21533, 21542, 21553, 21566, 21573, 21613, 21580, 21630, 21652, 21665, 21678, 21703, 21718, 21759, 21780, 21730,
    21799, 15815, 21821, 21833, 21845, 21858, 21871, 21884, 21896, 21909, 20570, 21924, 21932, 21949, 21960, 21972,
    21998, 13431, 13481, 13442, 22008, 22020, 22033, 22057, 22075, 22082, 22063, 22088, 22121, 22045, 22134, 22164,
    21636, 22185, 22204, 22225, 22242, 22258, 22267, 22278, 22287, 22247, 22296, 22305, 22313, 22321, 22330, 22354,
    22336, 22361, 22345, 21559, 21588, 21596, 22368, 22377, 21621, 21604, 22386, 21708, 22405, 20850, 20866, 22416,
    22445, 22485, 22508, 22532, 22544, 22584, 22588, 22595, 22619, 22640, 22629, 22664, 22675, 22688, 22717, 22736,
    22751, 22787, 22810, 22822, 22765, 22835, 22861, 22848, 22874, 22896, 22910, 22799, 22926, 22958, 22997, 23020,
    23039, 23070, 23078, 23084, 23102, 23111, 23121, 22391, 23137, 21713, 23153, 23166, 23194, 23220, 23202, 23275,
    23288, 23299, 23312, 23323, 23331, 23364, 23378, 23389, 23403, 23415, 23430, 23440, 23453, 23476, 23518, 23529,
    23542, 23553, 23566, 23575, 23586, 23599, 23614, 23642, 23624, 23671, 23694, 23719, 23678, 23701, 23743, 23754,
    23767, 23791, 23809, 23831, 23851, 23840, 23872, 23800, 23896, 23908, 23921, 23938, 23949, 14168, 23959, 23978,
    24005, 24014, 24033, 24064, 24087, 24109, 24134, 24143, 24154, 24164, 24174, 24226, 24242, 24262, 24307, 24354,
    24392, 24422, 24439, 24399, 24409, 24490, 24511, 5708, 15267, 24533, 24543, 24553, 24570, 24586, 24594, 24603,
    24616, 24628, 24683, 24721, 24738, 24748, 24758, 24770, 23423, 24791, 24803, 24817, 24836, 24849, 23489, 23465,
    23506, 24863, 24872, 24893, 24918, 24929, 24942, 24953, 24966, 24982, 24995, 25007, 25026, 25046, 25064, 25117,
    25153, 25054, 25072, 25125, 24562, 25162, 25172, 25216, 25224, 25233, 25241, 25250, 25259, 25271, 25284, 25322,
    25334, 25357, 25368, 19140, 25385, 19147, 25400, 25409, 25423, 25493, 25503, 25514, 25528, 25561, 25582, 25611,
    25619, 12564, 25628, 25653, 22101, 25675, 25709, 25739, 25750, 25762, 25784, 18400, 18430, 25801, 25822, 25876,
    25895, 25944, 25952, 25962, 25968, 25976, 25987, 25999, 25521, 26025, 26061, 26068, 26097, 26108, 26121, 26139,
    26153, 23968, 23987, 26173, 26181, 26191, 10718, 10733, 26197, 26219, 26244, 26292, 26257, 26327, 26342, 14828,
    26363, 26403, 26477, 26504, 26518, 26526, 26535, 26562, 26598, 26625, 26639, 26646, 23634, 23652, 24882, 18101,
    26653, 26660, 26666, 26703, 26722, 26713, 26732, 26745, 26764, 26774, 26782, 26797, 26809, 26829, 6463, 6477,
    26886, 26916, 26937, 26955, 18507, 12099, 26975, 27016, 27037, 27046, 9190, 1812, 27062, 27079, 27095, 27113,
    27125, 24271, 27180, 27228, 24498, 27241, 27254, 27286, 12783, 27316, 27331, 27340, 27350, 27296, 27384, 27137,
    27192, 27399, 27451, 27413, 27465, 27499, 27518, 27543, 27561, 27585, 27598, 25297, 8654, 8674, 27611, 27621,
    27633, 27649, 27663, 27695, 27710, 27674, 27729, 27744, 27761, 27777, 27804, 27820, 27844, 18515, 27865, 27889,
    27915, 27932, 27970, 28017, 28051, 28074, 28028, 28062, 28094, 28118, 28132, 28148, 28195, 28221, 28250, 28273,
    28236, 28295, 28336, 28369, 28391, 28415, 28425, 28438, 28465, 28452, 28479, 28515, 28525, 28537, 28560, 28347,
    27981, 27054, 26988, 28572, 27001, 28585, 28629, 28652, 28640, 28674, 28684, 28695, 28721, 28743, 28755, 27267,
    28774, 28814, 24523, 28838, 28857, 28876, 28902, 28913, 28550, 27996, 28006, 28926, 28936, 28948, 28959, 28969,
    28988, 29006, 27569, 27578, 29025, 29053, 29061, 26802, 29078, 29093, 29119, 29158, 29187, 29196, 29203, 29071,
    29220, 29232, 29246, 29284, 29308, 29315, 29322, 29340, 29384, 29394, 27706, 29430, 27687, 29465, 29434, 29479,
    29515, 29537, 8000, 29557, 29568, 29578, 29596, 29633, 29654, 29666, 29685, 29704, 29726, 29735, 29473, 29758,
    29784, 29810, 29840, 29855, 29876, 29883, 29891, 29903, 29928, 29943, 29954, 29935, 29964, 29975, 29984, 29994,
    30017, 30032, 30054, 30085, 14456, 30103, 30113, 30125, 30139, 30093, 30151, 30203, 30225, 30213, 30235, 30264,
    29607, 30303, 30316, 30331, 15051, 30389, 30409, 30425, 30441, 30503, 30526, 30581, 30513, 30609, 30671, 30732,
    30759, 30739, 30798, 30821, 30861, 30871, 30881, 13904, 30900, 30912, 30939, 30967, 30995, 31017, 30977, 31006,
    31032, 31047, 31093, 31111, 31183, 31201, 31220, 31244, 31232, 31193, 31255, 31263, 31284, 31322, 31273, 31332,
    31344, 19230, 31364, 31384, 31407, 31428, 31441, 31452, 31476, 31495, 31517, 31554, 31529, 31591, 31610, 31674,
    31698, 31719, 31747, 11015, 5511, 31779, 5554, 5632, 31807, 31788, 31830, 13786, 13798, 31870, 31896, 13810,
    31916, 31928, 5750, 31942, 5718, 31974, 31983, 31991, 32014, 32028, 32055, 32060, 31393, 32066, 32077, 32111,
    32122, 32130, 32139, 31420, 32152, 32171, 16841, 32185, 32211, 32248, 32190, 32263, 32281, 16970, 32304, 32224,
    32324, 32333, 32353, 32369, 32379, 32389, 32396, 32405, 32417, 32422, 32430, 32470, 32485, 32504, 32494, 32516,
    32554, 32563, 32572, 32584, 32593, 32601, 32616, 32624, 32633, 32643, 32608, 32653, 32661, 32670, 32690, 32702,
    32720, 32727, 32695, 32736, 32763, 32772, 32843, 32740, 32751, 32869, 32892, 32878, 32851, 32961, 32988, 33018,
    33053, 33062, 33090, 33109, 33030, 33099, 33118, 33128, 33140, 33071, 33202, 33151, 33081, 33227, 32967, 33238,
    33248, 33257, 33265, 33285, 33302, 33328, 33345, 33252, 33389, 33402, 33445, 33511, 33528, 33543, 33559, 33590,
    33603, 33644, 33671, 33698, 27278, 33721, 28765, 33739, 33751, 33520, 33351, 33765, 33777, 33575, 33618, 33630,
    33658, 33685, 33710, 33790, 33819, 33833, 33852, 33863, 33876, 33887, 33804, 33935, 33957, 2081, 33358, 33971,
    33996, 34015, 34038, 34089, 34099, 34112, 33394, 33366, 34148, 34165, 34182, 34220, 33375, 34256, 34313, 34325,
    34402, 34430, 32392, 34456, 34469, 34482, 34494, 34521, 34540, 34566, 34574, 34583, 34609, 34615, 34622, 34647,
    34657, 34669, 32399, 34689, 34707, 34742, 34757, 34764, 34771, 34778, 1828, 34790, 34800, 34809, 34825, 34851,
    34866, 34817, 34899, 34906, 34915, 34931, 34944, 34965, 35008, 35029, 35042, 34976, 35067, 35117, 35080, 35130,
    35093, 35155, 35166, 35179, 35190, 35199, 35209, 32408, 35218, 35230, 35246, 35257, 35268, 35285, 35301, 35307,
    35324, 35337, 35354, 35372, 35389, 35416, 35330, 35345, 35380, 35397, 35423, 35439, 35453, 35474, 35497, 32479,
    35515, 35526, 35555, 35573, 35589, 2740, 35601, 35627, 35662, 35689, 35710, 32577, 35744, 35761, 35777, 9412,
    35799, 35828, 35843, 35865, 31564, 35883, 35895, 35926, 35950, 35937, 35975, 35997, 35983, 36009, 36035, 36076,
    7610, 36092, 36114, 36119, 35431, 36145, 36156, 36173, 36196, 36224, 36242, 36266, 36288, 36304, 36319, 36329,
    36312, 36339, 36359, 36404, 36416, 36428, 36441, 36454, 31759, 36468, 36478, 36489, 36500, 36512, 36546, 36558,
    36577, 36619, 36632, 36233, 36254, 35406, 36645, 36684, 36714, 36749, 36767, 36790, 36820, 36832, 36655, 13245,
    36847, 36865, 36897, 36945, 29329, 35521, 36969, 36980, 36153, 36990, 37000, 28852, 37011, 37025, 24282, 37057,
    37087, 24292, 37067, 37097, 37036, 37077, 5889, 37118, 37145, 4338, 37177, 37195, 37185, 37203, 37229, 37264,
    13156, 37287, 37298, 37311, 37322, 37334, 37369, 37381, 37397, 37415, 37438, 37445, 37469, 37500, 37512, 37506,
    37530, 37547, 37585, 37591, 37599, 37621, 37629, 37668, 37711, 13166, 37746, 37801, 37822, 37753, 37847, 37854,
    28868, 37861, 37886, 37906, 37934, 37956, 37981, 37944, 38068, 38077, 38088, 38122, 38151, 38176, 38192, 38234,
    38201, 38260, 38243, 37636, 38282, 38298, 15015, 38333, 38342, 15025, 38361, 38290, 38382, 38400, 13739, 13861,
    38415, 38449, 13870, 36128, 38460, 38527, 38565, 38537, 38584, 38644, 38662, 38675, 38701, 38688, 16093, 10315,
    38729, 16130, 38751, 38765, 38780, 38817, 38829, 38841, 38884, 38901, 38909, 38922, 38941, 38955, 38997, 39030,
    39006, 37536, 37560, 29817, 39081, 39091, 16955, 37522, 39100, 39112, 39134, 39153, 12420, 39170, 3275, 39196,
    39205, 20890, 20899, 20936, 39226, 20910, 39238, 39271, 26035, 39307, 26044, 39338, 39353, 39374, 7105, 39394,
    39413, 39433, 39444, 39456, 39485, 39497, 39511, 39544, 39568, 39555, 39579, 39601, 37542, 37573, 38916, 39615,
    39634, 39658, 39669, 39682, 39703, 39735, 39109, 39760, 39790, 39805, 39823, 39846, 39932, 39983, 39856, 40011,
    40029, 39798, 40049, 40059, 40077, 40088, 40099, 40110, 39828, 40121, 40129, 40151, 40139, 40161, 40185, 40206,
    40192, 40216, 40225, 40240, 40266, 40278, 40232, 40291, 40314, 8270, 40327, 40251, 40351, 40379, 40414, 40431,
    40453, 40467, 40482, 16342, 16357, 40471, 40505, 40517, 40531, 40575, 40590, 40606, 40658, 40668, 40598, 40615,
    40054, 40680, 40695, 40713, 40734, 40750, 40789, 40826, 2963, 40839, 40879, 40924, 40938, 40983, 39813, 13694,
    41011, 41030, 15035, 38372, 41046, 41062, 39834, 41073, 6095, 41097, 41132, 6023, 41107, 41116, 41142, 41151,
    41184, 41213, 41223, 41249, 41287, 41259, 41306, 41297, 41270, 41340, 41376, 41394, 30451, 41406, 41432, 41448,
    41458, 41479, 41490, 41469, 41505, 3438, 41528, 41535, 41554, 41562, 41596, 41623, 41642, 41662, 41715, 41775,
    41814, 41837, 41828, 41851, 29334, 41869, 41892, 41880, 41909, 41934, 41916, 41160, 41925, 41955, 41975, 41987,
    22551, 42010, 42023, 42037, 42062, 42080, 42085, 42092, 16368, 42098, 42138, 42164, 29016, 42103, 42189, 42204,
    42233, 42246, 42264, 42279, 42290, 40806, 42302, 42320, 42349, 42362, 42377, 42385, 42330, 42239, 42016, 42394,
    42424, 37213, 42442, 42456, 37240, 42431, 42488, 42508, 42526, 42536, 42559, 42548, 42568, 42579, 42598, 41315,
    31462, 42633, 42669, 42693, 41652, 42707, 42763, 42810, 42846, 42868, 42890, 42956, 42979, 42821, 43017, 43042,
    42721, 42777, 42731, 42787, 42835, 42857, 42879, 43066, 43103, 43118, 43135, 43198, 43211, 43224, 43257, 43293,
    42517, 43325, 42904, 43348, 43399, 43416, 43438, 43448, 2566, 43459, 43469, 2578, 43480, 43508, 43489, 43529,
    43555, 43609, 43636, 43658, 43565, 43071, 43681, 43703, 43736, 43756, 43518, 43779, 43790, 43746, 43767, 43801,
    5730, 16114, 43820, 20589, 43868, 5584, 43887, 43909, 5597, 43934, 43950, 43965, 44010, 43973, 44064, 44094,
    44102, 44111, 44130, 44154, 44174, 44191, 28600, 44214, 44224, 44253, 44267, 12996, 16726, 16736, 44280, 44311,
    44324, 44334, 16019, 44234, 44347, 44379, 13882, 28086, 17682, 44396, 44422, 14203, 44436, 44449, 40832, 44476,
    44491, 20956, 44454, 44466, 44520, 20964, 44567, 44602, 44640, 44679, 44694, 44717, 44764, 44484, 44499, 44804,
    44815, 44810, 44840, 22191, 37717, 44852, 44855, 44859, 44865, 35446, 17105, 29644, 44871, 44881, 44907, 44912,
    35187, 44918, 44929, 44947, 44967, 44977, 44998, 45009, 44921, 45022, 45045, 8074, 45057, 45081, 45103, 44937,
    45131, 45146, 45179, 45186, 45194, 45204, 45220, 45271, 45288, 45299, 45308, 45318, 45324, 45276, 45329, 45340,
    45367, 45385, 45403, 45431, 45449, 45500, 45459, 45510, 45472, 45411, 45519, 45533, 45552, 45594, 45542, 45561,
    45571, 45627, 45690, 45729, 45751, 34333, 45766, 45776, 24988, 45785, 45796, 45815, 45824, 45835, 45873, 45919,
    45937, 45951, 45986, 45996, 46007, 46026, 46050, 46060, 46079, 46096, 46116, 46138, 46161, 14399, 16644, 46183,
    46214, 46240, 46259, 46284, 27903, 46272, 46321, 46332, 46367, 46381, 46397, 46417, 46432, 46249, 46345, 46355,
    45583, 46448, 46463, 46483, 46500, 46536, 46564, 46580, 46643, 46693, 46728, 46743, 46756, 46772, 46792, 46808,
    46735, 46821, 46836, 46853, 46908, 46923, 46930, 46939, 46955, 46947, 46964, 46973, 46984, 46998, 47013, 47025,
    47019, 47041, 11646, 47074, 47084, 47091, 47100, 47124, 47140, 47159, 47174, 47196, 47209, 47218, 47203, 47242,
    47250, 47227, 47259, 47276, 47293, 47309, 47385, 47418, 47455, 47466, 47484, 47512, 47520, 47526, 47578, 47593,
    47611, 47629, 32781, 47648, 47531, 47680, 47694, 47702, 47717, 47773, 47809, 47829, 47840, 47849, 47857, 47867,
    47877, 47887, 47926, 47955, 48024, 48053, 48073, 48086, 48096, 48109, 48117, 48126, 26570, 26579, 48029, 48058,
    48148, 26590, 48163, 48173, 48183, 48199, 48228, 48244, 47301, 48261, 48290, 48339, 48351, 48361, 48372, 48399,
    48445, 48494, 48510, 48529, 47898, 48543, 48560, 48579, 48597, 48613, 47907, 48629, 48643, 48651, 48659, 48717,
    48739, 48725, 48536, 48759, 48777, 48788, 48768, 48803, 48826, 48861, 48870, 48888, 48928, 48946, 48989, 48956,
    40947, 49033, 40957, 49079, 49096, 49143, 49161, 49179, 49220, 49152, 48937, 49234, 49243, 49280, 49289, 49300,
    41672, 47164, 49317, 49326, 49306, 49346, 49365, 49389, 49420, 49447, 49454, 49474, 49515, 49530, 45197, 49548,
    49560, 49568, 49606, 49632, 49639, 49650, 49578, 49661, 49688, 49716, 49746, 49757, 49770, 48898, 49802, 49815,
    49847, 49867, 49855, 49906, 49915, 49553, 49926, 49943, 49952, 49961, 49989, 50006, 50017, 50029, 50041, 49996,
    50050, 50060, 50088, 50112, 34874, 47619, 48807, 50135, 49968, 48830, 50155, 50172, 50187, 50162, 49810, 49585,
    48908, 50206, 49522, 49539, 50221, 50256, 50270, 50289, 47264, 50311, 50353, 50369, 50434, 50463, 50477, 50489,
    50513, 50532, 50545, 50559, 50575, 50583, 50592, 50611, 50626, 50670, 50617, 50686, 50703, 50721, 50733, 50693,
    50749, 50761, 47270, 50774, 50783, 50483, 50793, 50804, 50844, 50798, 50858, 50879, 50895, 38468, 50907, 50939,
    50969, 50985, 50997, 51009, 51049, 51071, 51092, 51103, 51055, 51117, 51133, 51153, 51165, 51176, 51195, 51211,
    51239, 46843, 51262, 51272, 51289, 51301, 51312, 51334, 51378, 51323, 51390, 51402, 51416, 51427, 51438, 51450,
    51470, 51514, 29099, 29126, 29165, 51525, 51555, 51580, 51610, 51077, 51533, 51625, 51639, 51617, 51696, 51727,
    51745, 51708, 51775, 51811, 51854, 51901, 51912, 48452, 47460, 47471, 47491, 51924, 51974, 52026, 46860, 52039,
    17529, 52096, 52110, 52140, 38268, 46991, 52160, 52174, 52181, 52187, 50013, 52248, 52271, 39316, 52296, 39321,
    52307, 52324, 52358, 39329, 52315, 52333, 52376, 52432, 52457, 52512, 52522, 52543, 52552, 52594, 52607, 52560,
    41346, 52631, 52644, 52638, 52660, 52671, 52713, 52756, 52778, 52796, 52809, 49104, 52824, 52846, 52867, 52881,
    16243, 52899, 52924, 52939, 52973, 52723, 25682, 52994, 53019, 53040, 53077, 53093, 25082, 53124, 53142, 53163,
    53208, 53253, 53276, 53359, 53373, 53390, 53414, 51564, 53424, 48295, 53456, 53467, 53477, 46570, 53486, 53501,
    53515, 53542, 53561, 53592, 53553, 53610, 46575, 52367, 53625, 53638, 53663, 53681, 53717, 53732, 53790, 53646,
    53653, 53526, 53810, 53817, 53570, 53672, 53725, 53827, 21430, 53844, 53850, 53860, 53871, 53887, 53915, 47131,
    53932, 53968, 29748, 53981, 54011, 54024, 54038, 54054, 54068, 54045, 54029, 54113, 11703, 54132, 9610, 46588,
    54122, 54148, 54165, 54182, 54217, 54254, 54286, 54329, 54388, 54411, 54428, 19481, 54478, 54535, 54558, 54612,
    54570, 54487, 54546, 54584, 54624, 54597, 54637, 54665, 54686, 54726, 54768, 54794, 54833, 54848, 46749, 54859,
    54882, 52167, 54897, 54905, 54914, 54954, 54973, 54991, 55006, 54998, 55013, 55037, 55048, 55065, 55074, 55082,
    55099, 55138, 55158, 55175, 55205, 55226, 55309, 55330, 55343, 55359, 55381, 55370, 55412, 55436, 55420, 51755,
    55457, 55471, 55483, 14982, 14955, 51862, 55534, 55428, 52046, 2189, 55563, 55576, 55648, 55491, 54922, 55694,
    55719, 55735, 55762, 55786, 55797, 55815, 55838, 55852, 54262, 46598, 55875, 55895, 55920, 55934, 55950, 55973,
    56002, 56014, 56043, 56079, 56088, 56097, 56109, 56121, 56139, 56188, 56206, 56150, 56222, 56275, 56298, 56316,
    56333, 56323, 56367, 56344, 56386, 56400, 56417, 56423, 56442, 56429, 56009, 56460, 56481, 56489, 56497, 37808,
    56507, 56523, 56449, 56547, 56555, 56570, 56409, 56563, 56600, 55091, 56621, 56639, 56648, 56683, 56709, 56722,
    54841, 56737, 56750, 56764, 56781, 56772, 56790, 56800, 56805, 56811, 56822, 45423, 45439, 22397, 56848, 56856,
    56866, 56877, 56888, 56905, 52251, 56927, 56957, 53493, 56979, 56995, 57013, 57031, 57064, 57072, 57090, 57109,
    57121, 53048, 57133, 57146, 57187, 57215, 57254, 57099, 57114, 57271, 57300, 57278, 57328, 57381, 57400, 57414,
    49112, 46606, 57447, 54400, 57469, 57538, 57556, 57567, 57580, 57590, 57598, 57609, 57615, 57627, 57656, 57670,
    57702, 57719, 57726, 57734, 57742, 57762, 57776, 57792, 57814, 57825, 57838, 57859, 57872, 57888, 57919, 57931,
    57944, 57966, 57988, 57997, 58005, 58015, 58024, 58047, 58077, 58125, 58084, 58132, 58094, 58103, 58114, 58140,
    58161, 58149, 58179, 58210, 58235, 58244, 58276, 8133, 58286, 58298, 58308, 58326, 58333, 58342, 58361, 58421,
    58439, 58453, 58469, 58512, 58589, 58606, 58649, 58667, 58187, 58218, 58682, 58254, 58698, 58721, 58734, 58747,
    58789, 58811, 58821, 58841, 58857, 58867, 40762, 58849, 58880, 58904, 58983, 59016, 52437, 59033, 59047, 59073,
    59092, 59121, 59145, 59160, 59038, 162, 184, 52197, 59196, 59215, 59229, 59239, 51476, 59255, 59262, 51482,
    59270, 59279, 59289, 59297, 59307, 59318, 59339, 59347, 59358, 59369, 59383, 59329, 59401, 59423, 57710, 9953,
    48104, 59434, 59445, 59463, 59427, 59479, 59488, 52786, 59498, 59508, 59519, 59531, 59548, 59568, 59582, 59598,
    59609, 59622, 59631, 59642, 59651, 59682, 59538, 59701, 59719, 59730, 59741, 59747, 59755, 59766, 58371, 59786,
    59792, 59807, 59839, 59872, 9657, 59882, 59812, 59900, 56831, 59914, 59800, 59930, 59941, 59935, 59957, 47712,
    56839, 59978, 59994, 59922, 60035, 60051, 60072, 60082, 24693, 60099, 60121, 60107, 57601, 60150, 60164, 60157,
    60173, 60181, 60198, 60214, 60236, 60247, 60297, 60202, 60318, 60339, 60362, 49186, 60373, 60387, 7025, 8253,
    60380, 60400, 7134, 60407, 60418, 60440, 21448, 60460, 60488, 60507, 60514, 60558, 60569, 60580, 60598, 60615,
    60606, 60647, 60672, 60707, 60746, 60767, 57636, 58317, 60787, 60857, 60077, 56747, 57224, 60894, 58617, 60957,
    58632, 60999, 61023, 61056, 61078, 61093, 61115, 61135, 61152, 61173, 61207, 61249, 61266, 61285, 61317, 61354,
    61373, 61389, 61398, 61411, 61426, 61447, 61438, 61465, 61489, 61500, 58756, 61529, 61589, 61538, 61595, 61548,
    61678, 61557, 37475, 61728, 61756, 61820, 61840, 61873, 61912, 61924, 61937, 61949, 61994, 62014, 62031, 62043,
    62057, 62077, 62110, 62126, 61602, 62148, 62158, 62170, 62180, 62022, 62198, 62215, 62226, 62237, 62249, 62271,
    62286, 62260, 62299, 62311, 62323, 62345, 62336, 62359, 62369, 62397, 50471, 62443, 62453, 62463, 62377, 62486,
    35458, 35465, 62534, 62550, 62581, 50067, 62602, 62616, 62622, 62629, 62674, 62693, 62701, 62708, 62724, 62778,
    62803, 62822, 62839, 62875, 62893, 53134, 61610, 62923, 62931, 62937, 62953, 63000, 63035, 63049, 63062, 63080,
    63096, 63119, 63129, 62494, 63141, 63161, 63180, 63104, 63192, 47723, 63208, 63225, 63232, 63186, 62679, 7032,
    63240, 63260, 62715, 63251, 63275, 63282, 63316, 63331, 63350, 63366, 48817, 49979, 63384, 63420, 63436, 63511,
    63520, 62962, 63527, 63533, 63540, 63568, 63608, 63630, 63656, 63679, 37484, 63707, 63741, 63760, 63797, 63815,
    63837, 63866, 63690, 63911, 47732, 63935, 63956, 63976, 47739, 63941, 63355, 64008, 64043, 64066, 64075, 62116,
    64093, 64108, 64119, 64138, 64154, 64191, 64218, 64244, 64286, 64309, 64334, 64226, 64353, 64371, 64318, 64235,
    64362, 64405, 64438, 64378, 44359, 64457, 64471, 64489, 47818, 64514, 64536, 64551, 64566, 64584, 64544, 64559,
    64595, 64610, 64676, 64728, 64740, 64755, 64774, 62636, 64799, 64815, 62644, 64870, 64890, 64908, 64899, 64922,
    64942, 64953, 64964, 64973, 64991, 65030, 64253, 65052, 65094, 65117, 65132, 65151, 65175, 65212, 65158, 65238,
    65254, 65275, 26076, 26086, 65261, 65293, 65320, 65328, 65338, 65346, 65356, 65375, 24182, 41682, 65407, 65431,
    65419, 65450, 65495, 65513, 65567, 65601, 65616, 65632, 65671, 65685, 65702, 65738, 65504, 54775, 54781, 14384,
    53865, 65755, 65772, 65789, 65831, 65778, 50915, 65784, 65916, 54788, 57559, 65964, 65985, 66007, 66021, 66040,
    66058, 66080, 66134, 64590, 66144, 66176, 66212, 66234, 66245, 60392, 47539, 66139, 66261, 66270, 66280, 66301,
    66335, 66290, 66311, 66322, 66345, 66353, 66368, 3798, 66383, 66397, 66414, 66432, 66422, 66445, 66463, 66506,
    66520, 66579, 66600, 41351, 9958, 10022, 41041, 66619, 59454, 66647, 66657, 66667, 66697, 66705, 66714, 66676,
    66723, 66734, 66687, 66748, 66765, 66773, 63448, 66787, 66813, 16036, 66838, 66849, 13454, 66910, 66934, 66960,
    48299, 50439, 67010, 67035, 67057, 67064, 67085, 66844, 66855, 67097, 66969, 67117, 67156, 67122, 65798, 65805,
    67192, 67204, 67219, 67236, 67253, 57804, 67226, 67274, 67298, 67307, 67318, 67327, 67337, 67347, 62132, 67374,
    67439, 46193, 48999, 67458, 67512, 67536, 10508, 67547, 67573, 67598, 67609, 67629, 67643, 67636, 67669, 67697,
    67715, 67737, 67779, 67825, 67877, 67918, 17490, 57897, 50444, 53420, 67939, 67953, 46871, 67969, 67996, 68025,
    68045, 68072, 68083, 68096, 68116, 68141, 62037, 68174, 67947, 68190, 68247, 68292, 68304, 68322, 68329, 68337,
    68343, 68351, 68358, 68366, 67161, 68379, 68397, 43359, 52653, 65248, 68465, 68473, 68483, 68504, 64620, 68520,
    68548, 68571, 68579, 68588, 68597, 68617, 68624, 68634, 53460, 68669, 68687, 68716, 68676, 68746, 68778, 68801,
    68811, 68786, 68821, 68836, 68852, 68862, 68883, 44181, 68889, 68901, 68919, 68933, 68908, 68990, 68999, 69027,
    69064, 69080, 69095, 69113, 61184, 69145, 69160, 69198, 69228, 69073, 58523, 69294, 58533, 69315, 23049, 23063,
    69308, 69335, 69364, 69412, 69426, 69444, 69469, 69506, 69525, 69531, 69538, 69558, 69570, 69576, 33261, 69583,
    69595, 69612, 69631, 69643, 33383, 48205, 69666, 69679, 69699, 32974, 69726, 69740, 69758, 69672, 69587, 34527,
    69781, 69794, 68844, 48211, 48237, 48218, 69806, 69786, 33826, 69564, 69819, 69830, 69842, 69851, 69863, 69917,
    46779, 69951, 69966, 69973, 70013, 69823, 70031, 69834, 70039, 70044, 53147, 70052, 70059, 68608, 70080, 70104,
    70162, 48134, 70173, 48501, 70191, 35294, 70231, 70256, 70242, 70277, 70319, 70335, 70292, 70306, 70349, 70364,
    70380, 70416, 70430, 70445, 70455, 70463, 70473, 70482, 70498, 64261, 65061, 64270, 65070, 65079, 70504, 70510,
    70517, 70533, 70525, 70541, 70549, 70559, 70571, 70586, 70604, 70577, 70617, 70659, 70676, 70703, 70623, 70726,
    70781, 70686, 70800, 70424, 70859, 70873, 70865, 35234, 70889, 70913, 58380, 70946, 70986, 71000, 70955, 71018,
    71044, 71030, 71056, 71070, 22700, 71085, 61471, 71106, 71132, 71148, 71112, 71171, 71183, 71202, 71287, 71303,
    66153, 71317, 71330, 71350, 48138, 71374, 41692, 71384, 71391, 71406, 71418, 71447, 71456, 71396, 71463, 41725,
    71486, 71497, 71514, 71524, 38355, 71492, 71555, 71561, 71607, 41699, 71645, 71295, 71310, 71670, 71691, 71708,
    71697, 71738, 71752, 71763, 71779, 71807, 71837, 71887, 71920, 71928, 71937, 71954, 71984, 67167, 72035, 72048,
    72062, 72081, 12515, 72105, 72124, 72137, 72150, 72169, 72207, 72234, 72258, 72307, 72245, 72338, 72350, 72372,
    72403, 72436, 72464, 16377, 72495, 64444, 72517, 71177, 72541, 72523, 72557, 72583, 72601, 72629, 72658, 72663,
    72671, 72683, 72725, 72769, 72789, 72838, 72854, 72875, 72883, 72865, 32790, 72899, 72922, 72943, 72960, 72978,
    72995, 73007, 72928, 72967, 2107, 72637, 72677, 72689, 72731, 73027, 73033, 72779, 48155, 73044, 72936, 62469,
    73089, 73151, 73195, 73208, 73216, 73225, 73234, 73242, 73250, 73265, 73290, 73303, 73322, 41606, 73341, 73363,
    70439, 73375, 25832, 73386, 73400, 73408, 17694, 70388, 17061, 73418, 73433, 73448, 73493, 73527, 73549, 73560,
    73571, 73532, 73542, 73579, 73554, 72532, 72891, 73585, 73606, 73625, 73646, 73679, 72091, 3544, 73690, 73698,
    73707, 64498, 65141, 73726, 73755, 73776, 73795, 73869, 73895, 73928, 56051, 73953, 73960, 73978, 73994, 74011,
    14608, 74047, 74059, 74078, 74084, 74095, 74111, 70165, 74153, 74179, 74185, 74193, 74200, 29406, 29417, 74220,
    74234, 74227, 72098, 74259, 74272, 74290, 74299, 74378, 74392, 74404, 46880, 74423, 74461, 74477, 74489, 74509,
    74534, 74548, 74561, 74575, 74590, 74606, 74567, 74618, 74581, 74596, 74657, 74678, 72566, 72574, 74690, 74705,
    74734, 59968, 74757, 74765, 74772, 74797, 74804, 74813, 73684, 74822, 71476, 74842, 74854, 68372, 74876, 74882,
    3619, 74895, 49043, 53056, 74914, 74933, 25688, 53000, 53063, 53069, 74904, 74833, 73393, 74978, 74994, 75010,
    75028, 75064, 75120, 75073, 75171, 75188, 75200, 75217, 75227, 75248, 75266, 75294, 65763, 75303, 75320, 75337,
    75327, 75349, 75364, 75379, 75392, 75407, 75419, 75434, 75465, 75481, 75455, 33311, 75508, 75525, 75238, 75552,
    75578, 75587, 74431, 15168, 75598, 75610, 75621, 75632, 51632, 75648, 75673, 75702, 75716, 75744, 75779, 75796,
    65815, 65823, 75822, 75851, 75866, 75872, 18548, 60471, 75882, 68896, 24186, 75906, 75929, 75949, 75966, 53938,
    75983, 76002, 76017, 76030, 76037, 76045, 55500, 76054, 76062, 76170, 76189, 76204, 55508, 76226, 76197, 55516,
    76241, 76255, 76272, 74713, 76294, 76319, 62687, 76331, 72695, 76349, 76365, 76377, 76424, 76442, 76463, 76484,
    76547, 76561, 76578, 76612, 64982, 76570, 76587, 71995, 76666, 76687, 76737, 76798, 76807, 76833, 76855, 64628,
    76875, 76068, 76917, 76931, 76942, 76990, 77028, 77074, 77106, 77133, 77112, 77157, 77170, 77181, 77192, 36136,
    68512, 64636, 44575, 76884, 77209, 56872, 77246, 77275, 77289, 77301, 16521, 77313, 77339, 77360, 77375, 77384,
    77393, 77417, 77442, 77464, 77479, 77501, 77527, 77579, 77595, 77616, 77622, 53898, 77630, 69436, 77638, 77646,
    69516, 77661, 77685, 77696, 77690, 77708, 77723, 53946, 77736, 77750, 77793, 77822, 77835, 77851, 77883, 77916,
    77925, 77933, 77942, 77829, 77702, 77379, 77960, 74848, 77974, 78002, 78017, 77449, 78032, 78040, 48918, 78050,
    43143, 78071, 78082, 78075, 52982, 78098, 78116, 78127, 78143, 78157, 78173, 78195, 78208, 69454, 78229, 78236,
    78246, 71758, 71769, 74000, 78270, 74213, 78283, 78295, 68404, 78309, 78367, 78380, 78314, 78391, 78409, 78421,
    78448, 78464, 78478, 64644, 78490, 78506, 78524, 68527, 78544, 2011, 78595, 78611, 75684, 78628, 78651, 76893,
    78699, 78661, 78721, 78740, 78794, 78816, 78830, 78851, 78866, 52339, 52350, 78884, 70180, 78893, 78415, 78602,
    68410, 78905, 78925, 55314, 4796, 74068, 54933, 54942, 55706, 78955, 55823, 78987, 78962, 79000, 79020, 79050,
    79007, 52802, 79014, 79065, 79119, 79131, 79142, 79156, 79168, 79182, 79204, 79213, 79232, 79294, 79327, 79343,
    79361, 79374, 79380, 79416, 79432, 79447, 79390, 79458, 79172, 79483, 79502, 79518, 45212, 79534, 79549, 79570,
    79560, 79583, 79595, 79623, 50814, 50825, 79642, 79653, 79665, 61192, 79685, 79713, 79723, 79734, 79793, 79824,
    79845, 79861, 79853, 79870, 79877, 79889, 79920, 79931, 79945, 79958, 63151, 79970, 79982, 79995, 80015, 80005,
    80025, 80053, 80071, 80099, 80116, 80124, 80133, 80140, 80160, 80174, 50743, 80184, 80208, 80190, 80214, 80225,
    80239, 80260, 80283, 48865, 80301, 80311, 80322, 80353, 80368, 80386, 80395, 80410, 80432, 80468, 80485, 80499,
    80523, 80533, 80561, 80576, 80595, 80613, 80625, 80646, 80668, 80657, 80692, 80744, 79134, 80771, 80802, 80814,
    80852, 80899, 80919, 80942, 80959, 80976, 80568, 81037, 81057, 81082, 81094, 81106, 81124, 81134, 81146, 81160,
    81151, 68050, 81170, 81194, 68058, 81178, 81215, 81231, 81251, 81279, 79145, 81321, 81336, 81351, 81285, 81368,
    81380, 81373, 81391, 81406, 81431, 81458, 81471, 81444, 81486, 81359, 81507, 81519, 81497, 81531, 81590, 81618,
    81598, 81541, 81650, 81677, 81712, 81682, 81744, 81768, 81788, 81795, 81810, 81826, 81839, 81927, 81945, 81962,
    81998, 81972, 81984, 82016, 82036, 82045, 82055, 81261, 82064, 82091, 81241, 82100, 81269, 82072, 20398, 82111,
    44139, 82131, 82190, 82196, 82203, 82225, 48548, 82244, 82263, 82284, 82304, 82334, 82349, 82357, 82366, 82385,
    82411, 82420, 82429, 82441, 82454, 82475, 82491, 82210, 82480, 82510, 60680, 82530, 82541, 55335, 55055, 55447,
    82570, 82581, 82593, 82606, 60715, 82624, 82648, 82672, 60726, 82535, 82688, 51343, 82715, 82726, 82772, 82780,
    82797, 82809, 82821, 82840, 82848, 82858, 82872, 82890, 82952, 82966, 83000, 82977, 83011, 83033, 82989, 83047,
    83059, 83077, 83088, 83100, 64682, 82552, 83121, 83140, 83202, 83067, 83240, 52119, 83151, 83158, 83253, 83267,
    83291, 83318, 82719, 83334, 83357, 83363, 83370, 83402, 83417, 83442, 83460, 83483, 83512, 83547, 83561, 83575,
    27835, 83469, 83594, 36803, 83607, 83642, 83652, 83662, 80195, 83690, 83707, 50495, 83720, 83738, 50503, 83728,
    83746, 83767, 83777, 83797, 48555, 57387, 83814, 83830, 83848, 83863, 83873, 83884, 40385, 83914, 60685, 83951,
    83980, 83672, 83993, 84010, 84025, 84017, 84032, 84051, 84082, 84103, 83713, 84123, 84141, 2778, 84152, 84164,
    84216, 84248, 84228, 84260, 84269, 84286, 84313, 84323, 84331, 84346, 84364, 84377, 84390, 84407, 84425, 84444,
    84461, 84485, 84496, 84509, 84519, 84530, 84339, 84550, 84566, 84584, 84604, 84624, 32906, 84655, 84687, 84709,
    84697, 84724, 84756, 84772, 84785, 84802, 84817, 84846, 84867, 84856, 84877, 84885, 84895, 84905, 84923, 84942,
    84958, 54338, 84980, 57480, 4154, 84999, 85034, 84574, 85051, 85058, 85067, 84416, 43082, 85085, 85097, 85109,
    85141, 85120, 85161, 73879, 85176, 85196, 85212, 85242, 85250, 85259, 85289, 85304, 85316, 85328, 85346, 85131,
    85361, 85391, 85404, 85413, 85427, 85437, 85446, 85457, 85466, 85477, 85488, 85500, 85511, 85523, 85542, 85550,
    85558, 85580, 85608, 85619, 85632, 85650, 85641, 85659, 85668, 85677, 85698, 85709, 85719, 85736, 85772, 47668,
    85789, 85820, 85836, 85854, 85873, 85883, 85918, 85936, 85972, 85991, 86023, 86042, 86070, 86086, 86102, 86135,
    86149, 86169, 86193, 86211, 86225, 86261, 86319, 86341, 86358, 86367, 65221, 86391, 86419, 85728, 86444, 25716,
    86501, 86523, 17903, 86543, 86561, 86578, 86587, 86552, 86432, 86569, 86597, 86625, 86634, 86642, 86661, 86701,
    45480, 86720, 86742, 86757, 86778, 86788, 86764, 86802, 85168, 85221, 86815, 86827, 85231, 86839, 86890, 86897,
    52907, 86919, 86934, 86952, 52853, 86998, 87014, 87038, 87054, 87070, 86651, 86670, 52665, 87098, 87143, 87157,
    87173, 16618, 87195, 87215, 87238, 87257, 87277, 87295, 87325, 87338, 87353, 87360, 87370, 87379, 87389, 87408,
    87425, 87434, 87444, 87467, 87479, 87455, 87492, 87499, 87506, 87520, 87547, 87565, 87594, 87610, 87625, 87639,
    87657, 87679, 87699, 87712, 87727, 87744, 50024, 87757, 87770, 87762, 87784, 87798, 87806, 87617, 87816, 87831,
    87854, 87870, 66819, 87886, 87918, 51824, 87937, 87951, 87959, 87968, 87979, 87990, 88004, 51836, 53366, 87944,
    88016, 88032, 88050, 88073, 88089, 88099, 70085, 88111, 88127, 88149, 88169, 88189, 88202, 88217, 88239, 88251,
    88263, 88286, 88296, 5238, 88307, 88328, 88366, 88381, 88410, 88437, 32798, 88456, 88474, 88489, 88514, 88526,
    88537, 88548, 88227, 88561, 69750, 72705, 46088, 82656, 88582, 88599, 88673, 88700, 88714, 88738, 88725, 88749,
    88761, 88781, 88803, 88791, 88813, 88839, 88851, 88863, 88872, 88883, 88274, 88115, 88904, 88919, 88936, 29488,
    88947, 88976, 89004, 89024, 89041, 89057, 89082, 46888, 21877, 89109, 89117, 89124, 89134, 89146, 53879, 89169,
    60258, 60304, 89182, 89204, 60312, 89176, 89271, 89289, 89308, 89299, 89318, 89280, 89328, 89344, 89361, 89378,
    89369, 89394, 80967, 89404, 89415, 81385, 89431, 89437, 89446, 89476, 7043, 89497, 89541, 89551, 86114, 89559,
    10673, 89582, 89606, 86122, 89628, 89647, 89665, 89678, 89697, 89712, 89731, 89750, 89757, 89765, 89773, 20513,
    89803, 89816, 89810, 89883, 80232, 79526, 89900, 724, 89913, 848, 1019, 79928, 83074, 89945, 89955, 90016,
    79952, 79989, 90070, 90076, 90086, 90106, 90114, 90127, 90147, 90174, 90200, 90181, 90221, 90241, 90263, 90280,
    90312, 90371, 90290, 90301, 90400, 90409, 90419, 90468, 90496, 90515, 90530, 90549, 90564, 50362, 90577, 90594,
    90609, 90631, 90554, 90649, 90669, 90675, 90689, 90699, 90713, 90733, 90740, 90749, 90779, 90798, 90808, 90844,
    61569, 90860, 90901, 90909, 90919, 90929, 90940, 90964, 239, 90988, 90999, 91013, 91024, 90867, 91038, 85745,
    91074, 91091, 91115, 91141, 91152, 91103, 91128, 91165, 91185, 91209, 90952, 90976, 91225, 91233, 91243, 91261,
    91274, 91285, 91316, 91346, 91367, 91399, 32, 91422, 91447, 91472, 91504, 91526, 91537, 91550, 91572, 91583,
    91607, 91623, 91634, 91647, 91666, 91679, 91701, 91722, 91747, 91767, 86452, 91784, 89948, 52147, 91798, 89673,
    91842, 91864, 91879, 91892, 91904, 91914, 45137, 45152, 91934, 91949, 91961, 80035, 80044, 80061, 80082, 91973,
    91981, 91995, 64145, 92012, 92021, 89153, 92031, 92051, 81333, 92071, 92094, 92104, 92114, 92163, 92199, 92210,
    92217, 92240, 92225, 36102, 92258, 92272, 92287, 92303, 92323, 92337, 92374, 92384, 1951, 92391, 92411, 88894,
    92429, 92442, 92448, 92457, 92469, 92490, 92504, 92516, 80635, 92534, 92553, 92542, 92563, 45350, 92575, 92592,
    45359, 92584, 92602, 92611, 92619, 92634, 61067, 92709, 80606, 92727, 92742, 21303, 92792, 92815, 92834, 92855,
    92870, 53534, 92899, 92845, 57392, 57406, 92916, 92937, 92975, 92982, 82500, 92997, 75178, 93012, 10516, 93003,
    93028, 93058, 93080, 93100, 93107, 93114, 93141, 93160, 55857, 93189, 93170, 93208, 93233, 92946, 93260, 93285,
    93316, 93331, 93360, 93381, 93413, 93432, 93345, 93371, 93463, 93489, 93529, 93550, 93573, 93636, 93682, 93266,
    82827, 87104, 93699, 93748, 93707, 93763, 93785, 93718, 93774, 93796, 93842, 93854, 93866, 93893, 62968, 93912,
    93940, 93948, 93957, 93973, 93998, 94007, 94017, 84452, 94060, 94095, 94121, 94132, 43147, 94153, 94162, 94171,
    94196, 94218, 94244, 52932, 75253, 78025, 94260, 12733, 94282, 62188, 94303, 94319, 94331, 42309, 94350, 56233,
    94371, 86605, 86617, 92249, 30753, 54349, 84991, 54420, 94392, 94417, 94475, 94126, 94502, 94531, 94554, 94137,
    80219, 92924, 94583, 94599, 94614, 94629, 94657, 94693, 94636, 94666, 24192, 94707, 94750, 93392, 45228, 45239,
    94766, 94784, 94834, 94855, 94867, 94878, 94931, 94968, 95040, 76950, 76691, 95063, 95085, 95103, 95127, 95158,
    54437, 95178, 95190, 54443, 11551, 95208, 95244, 95254, 95263, 95291, 69872, 95307, 95271, 66159, 78824, 95353,
    95379, 71189, 95444, 95482, 95450, 95511, 95542, 95574, 95587, 95594, 95607, 76697, 95627, 95642, 95659, 95667,
    95674, 95699, 95707, 95716, 95727, 95739, 95787, 95819, 95830, 95842, 95849, 95858, 95879, 30275, 95893, 95907,
    95924, 95943, 95967, 95985, 87580, 96000, 95299, 95281, 96013, 96022, 96037, 96048, 96058, 96094, 86459, 91791,
    25135, 95184, 72042, 77602, 96114, 36085, 95635, 89637, 95045, 92036, 96134, 95651, 96152, 96159, 96179, 96188,
    96199, 96208, 96268, 80983, 92042, 96213, 58292, 59844, 96287, 96310, 96331, 96295, 96355, 96220, 96373, 96384,
    66741, 96400, 96424, 96452, 84913, 96406, 96481, 96500, 96519, 96534, 96527, 45334, 96556, 96627, 96637, 96656,
    96679, 96689, 96697, 96703, 65, 96723, 96756, 96779, 96709, 96802, 96860, 96880, 96716, 96912, 96925, 96938,
    55348, 96947, 96955, 96964, 96973, 96984, 97028, 97041, 97056, 97062, 97073, 89031, 97085, 97101, 44987, 97114,
    97136, 97091, 97151, 53508, 97182, 97203, 95069, 97222, 97145, 97161, 97171, 76704, 95077, 97249, 97261, 97281,
    97315, 97339, 36274, 97351, 97381, 51199, 77347, 97404, 97413, 97465, 97483, 97493, 97515, 52732, 97537, 97502,
    52744, 97549, 97573, 97601, 97621, 97650, 97656, 97681, 97715, 97752, 97774, 97804, 97839, 92075, 97851, 97790,
    97879, 97890, 97902, 97917, 97932, 97961, 97983, 98003, 42215, 98028, 98062, 98074, 98083, 98099, 98122, 98156,
    83958, 38099, 98176, 98195, 98210, 98222, 61125, 98237, 68645, 98273, 98296, 98342, 98354, 98361, 80703, 98367,
    98396, 98415, 98427, 98439, 98466, 98486, 98506, 98530, 98538, 98545, 94028, 98555, 98576, 98593, 98617, 98633,
    98673, 98688, 98701, 98716, 98731, 79369, 98748, 98769, 98776, 98784, 98831, 98846, 98863, 98887, 98898, 98907,
    98935, 98973, 98990, 99006, 99024, 99036, 99055, 32678, 99074, 99101, 99111, 99127, 99171, 98753, 99195, 99205,
    99216, 92232, 74437, 82143, 99234, 99253, 99286, 98496, 99301, 99312, 99346, 99359, 49004, 99389, 99419, 65969,
    99451, 49875, 49890, 99491, 99507, 99528, 99536, 99553, 99572, 99585, 99598, 99564, 99607, 264, 99623, 99632,
    99649, 99656, 99663, 99673, 99683, 99696, 99708, 99716, 99730, 99749, 99768, 99775, 99791, 99807, 99783, 99799,
    99826, 99843, 99834, 99863, 99815, 99851, 99894, 58597, 99914, 99933, 97213, 99945, 99987, 82395, 100002, 100025,
    100055, 100101, 100165, 100196, 100222, 100228, 26545, 100242, 100258, 100288, 100313, 100335, 100365, 100387, 100412, 100424,
    100436, 100457, 100469, 68103, 100321, 100481, 100505, 20994, 21010, 100543, 100566, 100588, 89567, 100609, 100628, 100646,
    49697, 49706, 100654, 100661, 100684, 100712, 100773, 100793, 100801, 100810, 51125, 100830, 100852, 100890, 100859, 100912,
    100867, 59561, 100930, 100874, 100950, 100921, 97663, 100966, 100978, 100972, 100993, 101006, 101043, 101063, 101019, 101053,
    101073, 100985, 496, 91010, 97848, 101118, 101131, 101147, 101155, 90138, 101189, 101201, 62405, 101226, 101267, 101313,
    62413, 101325, 101338, 101356, 101405, 101414, 101425, 101466, 101506, 101515, 101526, 31103, 101565, 101594, 66829, 101609,
    101632, 101644, 101657, 101716, 101748, 74266, 75386, 101762, 101770, 101780, 101805, 101827, 101861, 101882, 83023, 101900,
    101920, 101940, 95680, 101956, 101966, 101981, 79693, 101995, 34682, 102004, 102018, 102035, 102043, 2818, 102052, 102072,
    102099, 4051, 102124, 101665, 101724, 102139, 102161, 9970, 102180, 98625, 88683, 102198, 102229, 102245, 102250, 102265,
    102292, 102305, 102277, 102319, 102333, 102351, 102150, 102371, 99369, 102390, 32070, 80201, 102407, 102432, 102467, 102477,
    102495, 102507, 102523, 102541, 102550, 5896, 37047, 102561, 102584, 102609, 101331, 102326, 102342, 9977, 102625, 102642,
    102631, 102653, 99994, 102698, 102722, 102740, 102768, 102800, 102814, 102834, 102848, 102858, 101434, 101479, 78837, 102874,
    102883, 78842, 88692, 102892, 102914, 102938, 102977, 103002, 103021, 51084, 51215, 103040, 103082, 17824, 103093, 103104,
    103158, 73967, 98182, 103175, 98229, 103198, 103214, 102011, 93090, 103254, 103306, 103265, 103348, 103355, 103366, 103378,
    103393, 88446, 103406, 103418, 103437, 103458, 103493, 103505, 103540, 103556, 103572, 103583, 103605, 103616, 63393, 103628,
    77587, 103642, 102920, 101123, 102946, 30433, 53168, 103672, 103708, 103729, 103771, 103206, 103785, 47051, 103818, 64807,
    103878, 103903, 103934, 103954, 103972, 104004, 60191, 104029, 75710, 75724, 75755, 75764, 104044, 104078, 104088, 64466,
    79302, 104097, 104112, 104126, 104135, 104145, 101443, 104154, 104161, 104102, 102472, 77119, 103372, 104180, 44706, 44729,
    44586, 44610, 77218, 104212, 104219, 104250, 77125, 104186, 14695, 104272, 104293, 503, 541, 549, 591, 600,
    645, 91035, 104312, 89904, 89908, 104322, 104342, 104364, 104387, 104395, 84000, 104411, 104421, 43157, 86928, 104433,
    104455, 104467, 104486, 104507, 104517, 104552, 104496, 103633, 104574, 104585, 20667, 104602, 104613, 104593, 104624, 104643,
    104656, 104678, 104737, 104771, 104803, 80306, 104827, 104840, 104852, 104832, 104865, 104890, 103031, 104907, 104922, 104914,
    104938, 104959, 104987, 104414, 105012, 105031, 105055, 105019, 105038, 104963, 105073, 105088, 105104, 105176, 105183, 105191,
    105204, 105197, 105222, 105233, 105243, 105253, 4538, 37722, 77042, 105263, 105297, 105327, 105337, 105348, 105364, 105387,
    105405, 105415, 38476, 105424, 105447, 98093, 105460, 105477, 105452, 105469, 105522, 105529, 105537, 51929, 105557, 105583,
    105547, 105609, 105621, 105633, 105650, 105688, 105705, 105613, 105727, 104351, 104374, 90522, 90682, 90788, 90820, 90831,
    105744, 105764, 105775, 105798, 105818, 105829, 105840, 98109, 105877, 80104, 80268, 80291, 87777, 105889, 105902, 105932,
    105955, 105997, 106011, 16882, 106032, 106048, 106060, 44402, 106103, 106071, 44412, 106130, 106082, 106154, 9983, 106168,
    106178, 106188, 106224, 106247, 106266, 106348, 106353, 48254, 106359, 106364, 106390, 106396, 106409, 106454, 106469, 106481,
    106505, 106564, 106600, 106611, 106624, 106671, 106693, 64098, 106711, 106721, 106740, 106775, 106792, 106804, 106827, 96813,
    106745, 68150, 106852, 106867, 106879, 106891, 68155, 106905, 22887, 106929, 106946, 106968, 107002, 107042, 107058, 107069,
    107081, 71785, 78008, 107105, 107093, 31571, 98280, 107137, 101909, 6105, 107153, 107304, 60323, 107324, 107343, 105625,
    107364, 107386, 107410, 107442, 107470, 107398, 107422, 107489, 107510, 107566, 107524, 107594, 107537, 107552, 107580, 6705,
    14934, 107615, 107625, 107636, 107659, 101754, 107674, 107689, 107715, 54269, 107735, 107753, 38186, 107766, 107784, 107812,
    107832, 107794, 107822, 33292, 107849, 104424, 107867, 57677, 62559, 107890, 107916, 107898, 107939, 107958, 107981, 107994,
    108009, 108028, 7740, 108049, 108064, 54695, 54735, 108095, 107878, 57685, 62567, 107907, 107924, 107948, 108018, 108038,
    7754, 108056, 108114, 108129, 54079, 108177, 100013, 108190, 108202, 32343, 108221, 108239, 108253, 108273, 108282, 108305,
    108294, 108328, 108376, 108387, 108393, 108407, 105374, 108421, 51874, 15715, 51765, 108434, 108491, 108525, 108536, 108562,
    108600, 108574, 108550, 108588, 108612, 14886, 108624, 108641, 108651, 77762, 77777, 108661, 36004, 108672, 108702, 61259,
    53222, 53433, 67070, 35991, 108717, 108743, 108794, 108803, 108811, 108835, 108853, 18604, 108819, 108876, 108895, 95520,
    108929, 108962, 108847, 23228, 108711, 23234, 31800, 31842, 108978, 108999, 108989, 109033, 109067, 109083, 109106, 109120,
    31059, 109134, 109155, 109176, 109191, 109206, 109223, 109261, 6558, 109311, 41020, 109322, 20707, 82730, 96492, 109336,
    84178, 109358, 104689, 109370, 109381, 47961, 109421, 109392, 109474, 109487, 109499, 109514, 15187, 88608, 109529, 109543,
    109213, 109557, 109567, 109579, 109625, 109655, 108400, 108414, 109680, 38790, 109734, 109751, 81398, 109769, 109814, 109844,
    109783, 109791, 109865, 109879, 109897, 21225, 100558, 109915, 109931, 109950, 109979, 110032, 110077, 110088, 92753, 110099,
    110123, 110144, 92765, 110111, 110166, 55587, 110188, 110200, 110213, 110271, 6046, 110292, 110305, 6267, 110330, 110371,
    6289, 110319, 110391, 110411, 110422, 110226, 110237, 110399, 110434, 110450, 110487, 110528, 52947, 110558, 39123, 110568,
    110580, 110612, 81691, 107775, 108632, 31541, 110653, 110683, 110668, 110719, 110753, 110794, 110818, 6503, 110853, 110882,
    110897, 110910, 110940, 80863, 38800, 74469, 82154, 82168, 82178, 110961, 110989, 111005, 111017, 111027, 111061, 111078,
    111104, 111133, 111112, 111178, 111201, 111233, 38551, 109692, 111256, 111267, 111292, 111325, 111348, 111365, 111386, 111407,
    111450, 111465, 30809, 111478, 111504, 110831, 111491, 110339, 111547, 111573, 111610, 6116, 6371, 111642, 80908, 111664,
    111698, 106576, 106635, 111653, 111725, 111779, 111792, 111803, 111814, 84238, 33407, 111827, 111845, 111859, 111905, 111925,
    111944, 111962, 111950, 112001, 54497, 87112, 112060, 112071, 112084, 112103, 112115, 112139, 112162, 112185, 112213, 112229,
    83259, 112247, 83278, 112261, 112275, 6187, 19042, 106981, 19054, 57155, 112309, 112329, 112350, 112391, 85007, 112418,
    112475, 107164, 107177, 107190, 112510, 112539, 112565, 14010, 112584, 6382, 21768, 112602, 112364, 112625, 112636, 112614,
    112647, 46617, 112671, 107334, 112697, 112707, 112720, 112739, 46628, 112761, 112845, 112865, 112886, 112916, 112927, 112962,
    112998, 113011, 113004, 113036, 113048, 113059, 113068, 113088, 113099, 113112, 113124, 113137, 113151, 98187, 113176, 113195,
    113210, 113223, 104630, 113238, 113251, 113244, 113263, 113290, 113305, 113323, 113335, 113364, 113399, 113375, 113409, 113385,
    112148, 112154, 113419, 113432, 113441, 113455, 113471, 113485, 113583, 113596, 113606, 113615, 113634, 113655, 113644, 113665,
    111417, 106588, 113684, 111426, 113705, 113723, 113714, 113732, 113778, 75357, 106040, 113789, 113814, 13703, 113827, 113879,
    113837, 113894, 113848, 113859, 113869, 113905, 80826, 107725, 106142, 113624, 113920, 113930, 112108, 113947, 113969, 16890,
    16902, 16916, 113990, 114020, 114057, 114091, 114109, 114136, 114147, 114169, 114199, 114248, 114262, 114284, 114337, 6128,
    114363, 114386, 69479, 89615, 114419, 114432, 114447, 114474, 114461, 114494, 114516, 114525, 114554, 114570, 114599, 86267,
    114609, 65637, 114619, 65649, 114631, 106197, 114643, 114667, 108940, 79424, 20677, 114686, 114712, 108952, 114735, 6394,
    114761, 62385, 114813, 105491, 114826, 65440, 114841, 89965, 93500, 114861, 114879, 104194, 110538, 114920, 114968, 110592,
    114932, 114992, 114947, 112399, 115033, 112428, 115061, 110728, 115082, 68656, 112441, 115095, 115118, 75937, 12270, 12290,
    12303, 12316, 115141, 110380, 115164, 115203, 10262, 115233, 115251, 88621, 115268, 115281, 115294, 115320, 115342, 115306,
    74306, 115367, 115412, 115448, 105782, 115515, 115428, 115464, 105805, 115531, 115544, 50378, 115564, 92776, 84278, 115607,
    115615, 108444, 115625, 10163, 105852, 115657, 115681, 10128, 10176, 115670, 115731, 10139, 115752, 115763, 90539, 115774,
    115821, 115871, 99044, 115888, 115911, 90030, 115936, 115965, 115991, 112377, 67677, 116046, 116076, 28309, 116104, 28162,
    110738, 88632, 116142, 116162, 112551, 112683, 116090, 20298, 31121, 116186, 116210, 116259, 12167, 116296, 116332, 116344,
    116174, 112011, 112024, 116356, 112037, 116379, 31133, 36725, 116414, 116440, 116459, 116475, 116493, 6515, 116448, 116517,
    116539, 74697, 116574, 116593, 31145, 116617, 109347, 116638, 116663, 91734, 116706, 116731, 116761, 116674, 116690, 116778,
    103275, 116813, 107700, 116833, 116851, 116877, 116906, 116926, 116951, 116916, 76234, 117003, 117013, 73296, 73309, 87120,
    38595, 117024, 117045, 117063, 117054, 117072, 110965, 112319, 117093, 117138, 117161, 117182, 79075, 117171, 117201, 117219,
    117192, 117244, 117281, 112191, 112219, 112235, 106646, 79466, 60059, 117310, 60066, 117317, 29829, 117323, 117349, 117367,
    117328, 117379, 96822, 96871, 117392, 117415, 117386, 117435, 80179, 117449, 5952, 111438, 21743, 107015, 117471, 117506,
    117527, 117540, 31578, 76299, 117553, 117594, 117603, 117563, 117612, 117631, 117651, 117573, 117583, 84826, 117672, 117617,
    22964, 117690, 4670, 76303, 117709, 117719, 117759, 106700, 29439, 117780, 117805, 117855, 117878, 117908, 117916, 117944,
    117966, 117976, 117986, 117999, 22175, 118013, 118031, 13489, 118046, 22231, 118021, 118062, 118070, 118078, 118004, 118084,
    118094, 118122, 118138, 99758, 118180, 118198, 118217, 65270, 118237, 118250, 118257, 118263, 118271, 118279, 118290, 118303,
    86533, 118318, 118342, 118372, 118384, 118397, 118409, 118432, 118458, 118478, 64525, 118499, 118522, 118543, 118566, 118488,
    118580, 74120, 42197, 118622, 94068, 118639, 65457, 65574, 118665, 65466, 107500, 118715, 118738, 65583, 118765, 118677,
    118629, 79698, 10086, 118784, 118814, 40536, 3357, 118845, 21809, 118867, 118901, 118937, 118960, 118982, 118992, 119003,
    15833, 119015, 119042, 23729, 119082, 109825, 76434, 100635, 107201, 107215, 119144, 89505, 103791, 119167, 119185, 119201,
    119175, 119217, 72798, 115356, 119244, 119265, 119288, 119345, 119276, 69343, 119379, 119393, 119410, 119385, 119402, 119430,
    119474, 119488, 119481, 119503, 119545, 119558, 119516, 119532, 119574, 119584, 8453, 8468, 119437, 119599, 119608, 15347,
    119617, 119629, 119639, 119649, 119661, 119675, 119687, 119697, 61276, 119707, 119721, 119733, 119742, 119756, 119787, 53264,
    119806, 119817, 119828, 119865, 119878, 119841, 119892, 119852, 119903, 39466, 119916, 119941, 119929, 119749, 120005, 120056,
    120065, 120076, 120092, 120102, 110501, 120117, 120150, 17623, 120170, 120179, 120189, 120199, 120214, 120223, 105865, 120239,
    94776, 77201, 120269, 120276, 120283, 120290, 7203, 7212, 120298, 88338, 108121, 120316, 56581, 120334, 12374, 12382,
    120354, 12393, 120365, 120375, 120418, 120472, 27029, 75655, 120491, 120509, 120571, 120608, 120638, 28178, 120662, 120682,
    120704, 120725, 120749, 53580, 120787, 120760, 120524, 120624, 28259, 120815, 120836, 120650, 120857, 20313, 120873, 120887,
    120537, 120774, 120904, 121024, 121054, 34461, 121085, 121066, 121099, 121119, 117768, 121142, 113676, 121159, 121181, 121189,
    121198, 71532, 71503, 71540, 121212, 121231, 121249, 121266, 121278, 121289, 94311, 121302, 121318, 121334, 121349, 111674,
    121373, 121308, 121324, 121396, 121404, 121411, 121430, 121440, 111090, 44021, 121450, 43988, 121475, 121495, 121505, 121518,
    79025, 121535, 121587, 89214, 121608, 121632, 121669, 89221, 89229, 121682, 79033, 67017, 67022, 89237, 89244, 89252,
    121698, 121705, 89189, 79042, 121690, 121711, 121727, 121742, 121755, 121774, 105379, 105397, 121746, 121783, 121805, 121824,
    120194, 120204, 121837, 121851, 99033, 83054, 121875, 121889, 121925, 121937, 121897, 121880, 121958, 121976, 19096, 94103,
    48379, 121997, 122013, 77144, 122029, 122060, 122068, 122080, 122098, 122131, 122165, 60005, 122186, 122200, 122218, 36663,
    122208, 122249, 122316, 122324, 122349, 122370, 102592, 122397, 102600, 122405, 122414, 122442, 122453, 122473, 122493, 122519,
    122532, 70695, 116223, 122547, 122646, 93917, 122659, 122690, 122702, 82738, 122733, 122781, 122791, 122801, 122812, 47748,
    122832, 122843, 122853, 122859, 122421, 122868, 122883, 122908, 122924, 122960, 122932, 122942, 122968, 122996, 121638, 123024,
    25433, 123036, 123052, 122951, 122977, 123005, 123083, 123102, 70810, 123118, 123135, 123169, 123196, 36369, 123215, 123250,
    51223, 123265, 123290, 123301, 123322, 123343, 123368, 123397, 123405, 120012, 123414, 19627, 25694, 123435, 123456, 123471,
    123486, 123495, 55113, 55149, 55125, 123504, 123529, 123478, 74398, 121644, 123030, 123555, 123583, 123044, 123617, 123629,
    123641, 123651, 123659, 123678, 123688, 123698, 123722, 51786, 51798, 123705, 123729, 123738, 123748, 122696, 123759, 123785,
    123804, 123817, 123832, 94039, 123858, 93647, 93924, 102204, 123870, 123883, 120218, 123898, 111737, 123911, 123950, 123969,
    123981, 122708, 124021, 124035, 124064, 124027, 124076, 86466, 124098, 124123, 124136, 121650, 124152, 124205, 124250, 124286,
    124303, 124336, 124367, 124406, 124376, 124428, 124448, 124486, 73658, 73669, 124521, 124532, 9203, 124545, 124555, 124567,
    124585, 124600, 124619, 124642, 124658, 124673, 124664, 124690, 124698, 124709, 124735, 124753, 124775, 124764, 124786, 124797,
    124813, 124841, 55021, 74442, 123920, 124860, 124871, 124940, 124881, 39695, 119193, 124969, 116002, 125002, 47603, 125026,
    125047, 125083, 125101, 125143, 125168, 125202, 86849, 125223, 125252, 121948, 125266, 125232, 99395, 125057, 86858, 125290,
    125242, 86868, 125307, 125330, 125351, 125363, 125398, 125412, 116234, 83897, 125314, 125322, 69599, 125455, 125483, 125489,
    119624, 125514, 125526, 78674, 65086, 125537, 125564, 125584, 125601, 125619, 125637, 71363, 125656, 125646, 125670, 125687,
    125714, 125743, 125724, 49050, 125763, 125773, 123877, 125791, 76741, 107114, 125807, 125830, 124822, 76712, 75399, 105270,
    107122, 56465, 122429, 125846, 125884, 125894, 125920, 125947, 95390, 125968, 126000, 126011, 126023, 126031, 126041, 126058,
    55598, 96318, 101234, 126084, 126123, 126163, 79243, 126233, 79251, 126241, 126285, 126297, 126136, 126177, 126191, 126207,
    126219, 126150, 126310, 126322, 126334, 126358, 83210, 126388, 83222, 126413, 126459, 126505, 126342, 126475, 126400, 126518,
    126491, 80068, 113891, 117716, 126531, 126552, 126573, 126579, 126603, 126624, 126634, 126643, 67649, 86905, 90569, 126651,
    126676, 126683, 126692, 126660, 126667, 70109, 126706, 95050, 33942, 126720, 126754, 126765, 126772, 126789, 126804, 126835,
    126857, 126869, 126715, 106730, 61580, 126880, 32437, 32448, 101083, 15894, 126900, 126918, 101094, 108677, 56591, 126943,
    44617, 126968, 44776, 126979, 108970, 126988, 6682, 127004, 127030, 127065, 127072, 56730, 102414, 122256, 127082, 60242,
    127095, 127128, 127138, 103563, 127149, 127142, 22410, 44001, 127172, 127178, 127185, 127200, 127192, 127224, 127232, 127241,
    127259, 127276, 127287, 127299, 127307, 127315, 127335, 20112, 127372, 114486, 22003, 127405, 127410, 127423, 104528, 127381,
    14514, 118297, 127443, 127466, 127494, 127500, 99626, 127508, 127514, 127558, 127610, 104315, 106414, 106459, 127646, 127670,
    127680, 124084, 106419, 127695, 127687, 126065, 127714, 127734, 127743, 127755, 127768, 127808, 127823, 127762, 74890, 127863,
    127882, 127909, 127933, 3631, 127956, 127964, 68416, 127971, 128013, 128028, 128041, 128053, 128020, 68427, 128035, 128065,
    128077, 128093, 128107, 128122, 128148, 128132, 128167, 77424, 128183, 128194, 128207, 128215, 128224, 83377, 128231, 128240,
    128250, 128261, 62973, 63009, 128279, 128317, 128339, 128366, 6860, 102514, 128391, 128403, 128416, 128425, 128442, 127090,
    128454, 128468, 85268, 128488, 128506, 128543, 128558, 128565, 128573, 128594, 128609, 125152, 128641, 96143, 87513, 128602,
    128661, 128676, 69709, 62608, 128695, 128718, 128781, 128788, 128071, 271, 100642, 90875, 128799, 302, 449, 457,
    309, 351, 359, 398, 407, 128817, 128827, 128836, 128846, 128869, 128885, 104997, 128911, 128924, 128900, 128935,
    128956, 128973, 128997, 129030, 129042, 129056, 129068, 58192, 58223, 129087, 129114, 129121, 129130, 58260, 129152, 129177,
    129185, 129195, 129204, 129213, 121759, 129224, 129251, 99702, 129270, 129279, 129289, 129300, 129310, 129322, 129337, 129353,
    129365, 129370, 129379, 129386, 129396, 82292, 82313, 129416, 129433, 129451, 36296, 129425, 127133, 100172, 100203, 129487,
    51937, 129513, 129538, 129547, 129521, 129529, 129557, 129582, 129602, 29695, 129615, 129638, 129674, 129656, 129693, 129710,
    129722, 129736, 129752, 129768, 129778, 104326, 129758, 129790, 129804, 129816, 129832, 129847, 129889, 129897, 129855, 129933,
    129968, 129939, 129952, 129974, 129990, 129998, 130018, 130032, 130070, 130098, 130110, 130120, 105755, 130128, 130142, 130158,
    130171, 130175, 130180, 130204, 130218, 130225, 130234, 130266, 130281, 130307, 130315, 130330, 130363, 56392, 130373, 130387,
    130241, 130402, 130422, 130439, 130467, 130505, 130526, 21358, 130545, 130562, 130135, 130577, 130512, 130625, 130659, 130680,
    130693, 130712, 130730, 130778, 130792, 130812, 130858, 130876, 130889, 59850, 130905, 130933, 17119, 130945, 130882, 61618,
    130953, 130980, 130993, 131012, 61623, 127775, 131022, 131030, 131041, 131049, 131075, 131094, 131113, 131080, 131057, 131128,
    131150, 131160, 131173, 64128, 131194, 53181, 131221, 131204, 131232, 131253, 131280, 101320, 131297, 47639, 131313, 131321,
    131155, 131166, 131331, 131345, 131402, 131445, 131477, 131493, 131504, 131528, 131511, 131551, 131597, 131619, 131631, 131644,
    41785, 66185, 131669, 131700, 55727, 131716, 131766, 131787, 130958, 131803, 61630, 95196, 131813, 131846, 25636, 131870,
    112196, 122807, 131818, 131890, 131920, 131937, 131968, 105565, 105591, 130717, 130554, 131352, 112200, 131982, 131990, 132000,
    132017, 132008, 132029, 132058, 132076, 7279, 64013, 132093, 132106, 132117, 68557, 132131, 41731, 132153, 132179, 132185,
    132191, 130686, 132203, 132214, 132228, 92497, 132265, 132280, 132304, 132325, 132347, 132364, 132391, 132425, 62503, 132456,
    132485, 132496, 132515, 132547, 132577, 132613, 132625, 66388, 132644, 68197, 132658, 132679, 132701, 132727, 132773, 132816,
    49399, 132830, 132847, 55743, 131724, 120128, 132858, 131731, 132873, 132891, 132901, 132909, 67263, 132924, 132938, 11454,
    119227, 132955, 132981, 132998, 34716, 133017, 133038, 34749, 133074, 133159, 133187, 133215, 12429, 133268, 133313, 97289,
    133328, 97298, 133337, 66779, 133347, 133364, 133321, 132928, 133397, 3, 133413, 130285, 133461, 133467, 130581, 130591,
    130734, 130744, 3803, 133480, 133500, 85203, 54644, 54674, 133512, 133532, 133576, 37829, 133598, 133607, 133625, 21153,
    133644, 122229, 54451, 133676, 5355, 133709, 133724, 133736, 133764, 133750, 133779, 133794, 133813, 133834, 132488, 133858,
    133875, 133922, 133934, 133944, 133957, 133972, 133982, 133995, 134017, 134036, 124576, 111588, 134045, 106836, 134058, 134072,
    134092, 66473, 38425, 134113, 38437, 134136, 13889, 116153, 134152, 133401, 134163, 118853, 134175, 134216, 134183, 28209,
    134028, 54655, 23883, 133518, 134237, 10995, 134259, 119765, 134293, 53440, 134317, 134330, 134338, 47030, 134356, 134370,
    134389, 31294, 134379, 123222, 133222, 134407, 6649, 134430, 96167, 114159, 8402, 134466, 134484, 134505, 134524, 134539,
    91956, 62786, 134572, 81850, 45637, 132634, 134603, 134664, 134685, 62811, 134710, 134729, 134746, 134764, 134780, 134804,
    134852, 134874, 134892, 134929, 134772, 134792, 134950, 134820, 134863, 134968, 110248, 135006, 135039, 135048, 135070, 135094,
    135115, 135151, 135182, 135205, 135231, 103799, 135258, 135271, 135307, 135324, 135341, 135358, 43031, 135387, 135410, 94051,
    56022, 135433, 56033, 135455, 135469, 135479, 115944, 135490, 135507, 135546, 97423, 135584, 135601, 135625, 119299, 133084,
    135643, 135678, 135692, 132963, 133095, 132989, 119255, 133007, 33338, 135706, 135719, 135731, 135751, 135594, 135779, 135785,
    135793, 64994, 135821, 130294, 130601, 130611, 130754, 130764, 134065, 135852, 132616, 135875, 135886, 135897, 135952, 82959,
    136010, 136040, 80782, 86050, 136058, 136089, 136117, 136104, 136132, 136168, 136186, 136209, 136045, 135963, 136234, 135911,
    54357, 136244, 136255, 68007, 69103, 136291, 136324, 136301, 136348, 136378, 136400, 136424, 136462, 136481, 136524, 98791,
    136548, 136570, 136558, 136603, 136615, 136624, 136632, 136649, 19562, 103829, 136689, 136717, 133804, 136749, 136800, 136812,
    136822, 136840, 136850, 136830, 136862, 136880, 54505, 54514, 70816, 136922, 136936, 136955, 130273, 136970, 136998, 28782,
    137017, 137038, 137050, 131064, 131139, 137063, 130248, 137091, 137122, 137131, 137152, 137221, 137249, 137279, 137301, 137328,
    137351, 137400, 137363, 137413, 137452, 121787, 121813, 137490, 137505, 137520, 137541, 137550, 133965, 137566, 137581, 137591,
    137603, 137615, 33162, 137626, 33173, 33182, 81861, 81873, 81885, 137638, 137658, 137670, 131305, 137716, 137736, 137727,
    137756, 137779, 109706, 137798, 137816, 137827, 109716, 137808, 137840, 137861, 137880, 56474, 137892, 137899, 137907, 137923,
    47179, 47188, 120385, 109235, 109248, 137937, 137947, 129624, 92640, 137958, 137983, 138008, 138042, 37838, 78969, 133616,
    138054, 138075, 138129, 138156, 138176, 138089, 138192, 138209, 107856, 138229, 136360, 136390, 138257, 138267, 111746, 138287,
    138339, 138367, 138377, 138394, 138409, 138424, 138451, 54524, 138433, 138466, 138477, 138499, 138539, 119310, 138563, 119322,
    55237, 55249, 55261, 138601, 55273, 138614, 138640, 55286, 138627, 138681, 138695, 138722, 95552, 138771, 138801, 138818,
    134533, 138841, 138859, 138876, 123109, 69813, 1925, 53743, 123231, 136435, 136412, 138904, 4547, 24363, 118224, 128435,
    128516, 138947, 138961, 138974, 138954, 118230, 138989, 139000, 139023, 139060, 139128, 139142, 139155, 139166, 134193, 139177,
    136334, 139203, 139241, 139255, 1934, 30160, 134755, 139269, 139286, 139302, 139338, 139350, 139362, 34830, 34840, 135762,
    139388, 139403, 34006, 121421, 139438, 139457, 139476, 139503, 139536, 139489, 139516, 122875, 139562, 45807, 50073, 139579,
    139600, 139618, 139590, 139663, 139678, 50394, 139695, 50408, 139741, 139773, 139804, 139839, 139879, 117883, 139941, 43334,
    42915, 139962, 139971, 139982, 14420, 140012, 140025, 140041, 140065, 50095, 140078, 140128, 140141, 140158, 140198, 140216,
    140246, 140268, 140291, 137029, 140318, 102483, 140334, 140345, 140356, 133884, 140376, 135398, 135371, 140396, 133896, 110864,
    140418, 133910, 140438, 140366, 2473, 3022, 139750, 138652, 130821, 3448, 133371, 140492, 140507, 140520, 61637, 140530,
    140544, 140560, 132972, 26270, 26305, 125956, 140584, 2393, 135654, 2401, 135662, 140610, 53991, 130254, 137097, 140625,
    140646, 140662, 140680, 140741, 100346, 140767, 140783, 125014, 140798, 116424, 50105, 140822, 140828, 140839, 44285, 140857,
    140882, 53633, 133422, 133436, 135606, 140907, 140930, 140943, 140958, 140969, 140921, 135616, 133931, 130566, 130104, 130368,
    140979, 140985, 140991, 141016, 137160, 140689, 137169, 140698, 141044, 141063, 141087, 141109, 141119, 141131, 141149, 141172,
    37760, 57661, 141198, 141211, 141226, 141236, 141248, 141262, 141253, 141286, 141296, 141318, 141306, 141328, 9307, 141355,
    141371, 141393, 141414, 105113, 141434, 141456, 141474, 141492, 141503, 141514, 141533, 141543, 141553, 141564, 141575, 141586,
    141598, 141608, 45612, 141603, 141629, 141645, 141665, 141672, 141679, 141693, 141715, 141726, 141702, 141739, 141751, 126629,
    141766, 93177, 57955, 132581, 57977, 58035, 141775, 141797, 141819, 141839, 141870, 141883, 38487, 141826, 141846, 141833,
    141853, 141877, 141894, 141904, 141926, 141932, 141964, 141978, 142006, 140429, 42970, 142027, 42924, 42993, 142017, 142046,
    142055, 132551, 142066, 142089, 142097, 142104, 142115, 132519, 132562, 142138, 141771, 142157, 142180, 142214, 57341, 142223,
    142233, 142243, 142275, 142283, 142292, 49253, 139027, 139064, 139083, 142303, 142313, 142323, 142338, 142359, 142388, 142411,
    142425, 142436, 142463, 142480, 142471, 142543, 142561, 142606, 142571, 142552, 142630, 136818, 142649, 142669, 142684, 142702,
    142724, 142738, 142753, 142777, 142791, 130165, 142828, 142855, 142865, 142875, 142894, 142914, 142924, 142934, 142955, 142980,
    143008, 138015, 143024, 143065, 142786, 142800, 143087, 78454, 130782, 143115, 143125, 53752, 143162, 143170, 143178, 143207,
    143235, 130078, 143258, 143269, 143283, 143307, 143342, 130633, 85863, 143363, 143371, 143388, 143395, 143403, 143412, 130898,
    143421, 143446, 40993, 55215, 143467, 134419, 48666, 143505, 143559, 143577, 143597, 143607, 143618, 128963, 63323, 137845,
    143639, 136608, 143659, 54295, 143683, 143740, 143760, 143767, 143773, 143793, 99879, 60131, 143814, 143823, 142253, 142264,
    143833, 143845, 143853, 129234, 143861, 55904, 143134, 143878, 143900, 143931, 143966, 143987, 143998, 143975, 144011, 144031,
    75695, 144067, 17497, 144092, 144101, 120345, 139673, 144110, 144204, 144244, 144214, 144258, 11205, 144226, 144285, 144273,
    144299, 83166, 144311, 112094, 144335, 144115, 16600, 144352, 122664, 144372, 144382, 144396, 144417, 144433, 124496, 144454,
    144478, 138827, 144499, 144540, 144574, 144601, 134612, 144646, 144689, 144718, 144742, 80443, 144765, 144782, 144827, 144846,
    144873, 144883, 144908, 144925, 144937, 144954, 144974, 145000, 145012, 145024, 31687, 145055, 145075, 145098, 145116, 33269,
    145134, 145153, 145170, 10036, 118912, 145197, 118924, 145230, 145266, 145239, 145247, 145284, 145298, 145319, 134625, 145335,
    145348, 145372, 145399, 145422, 145452, 3576, 112770, 145470, 145510, 145433, 143689, 145532, 145571, 145582, 145595, 145623,
    145631, 145643, 65522, 145272, 137375, 145689, 145713, 37132, 37157, 36694, 145734, 130800, 145760, 100600, 145066, 143910,
    144614, 145800, 145821, 145845, 145087, 145863, 145876, 9056, 145890, 145920, 142374, 145905, 145944, 145969, 145997, 141403,
    143035, 146023, 115577, 146037, 111873, 146125, 132945, 65535, 146165, 146194, 146237, 146286, 146206, 146305, 146325, 146344,
    146376, 135556, 62793, 135570, 146253, 146410, 145772, 146430, 146486, 146527, 146572, 82521, 102985, 146597, 146670, 53283,
    146698, 146711, 145654, 146723, 146751, 137387, 137426, 146769, 146788, 146811, 146822, 146703, 72810, 146859, 146882, 146869,
    146901, 146928, 145782, 146891, 146914, 146945, 146761, 124744, 146179, 146963, 28492, 46297, 36346, 147024, 147047, 144756,
    147055, 112783, 147064, 144878, 144888, 144913, 147096, 145257, 147075, 76922, 147112, 147132, 147167, 143702, 137074, 15060,
    30399, 145106, 145181, 147188, 147205, 147224, 112794, 112854, 147250, 144727, 93901, 147271, 147290, 147303, 18108, 112874,
    138662, 139011, 147316, 147346, 147357, 139609, 139627, 147383, 147402, 147436, 147451, 147409, 147467, 147494, 147529, 142581,
    138732, 147553, 137440, 147573, 147587, 138385, 89823, 32081, 147608, 147640, 147664, 147695, 147711, 112971, 34726, 133027,
    116556, 108456, 108469, 144464, 147728, 147755, 147795, 147817, 147833, 115553, 44506, 147867, 144660, 146584, 102235, 147916,
    142616, 142592, 147973, 115692, 147997, 148030, 148009, 148051, 115702, 148020, 148081, 148111, 148130, 148157, 147927, 147418,
    147476, 57308, 133104, 148182, 148214, 148240, 133115, 148263, 148251, 144773, 148290, 148301, 145668, 145811, 148329, 39713,
    148350, 148365, 148381, 19811, 148411, 148426, 148449, 146612, 148480, 148521, 148532, 133866, 145854, 148545, 148570, 101793,
    148589, 147330, 148609, 148670, 148554, 148706, 115951, 148741, 135516, 148755, 2420, 148798, 148809, 148818, 147766, 148841,
    148870, 148890, 148965, 138298, 148902, 148993, 100446, 149021, 149040, 147540, 19254, 149052, 149064, 149092, 149101, 15195,
    149126, 70825, 147777, 149140, 148914, 26964, 149162, 145483, 149178, 149195, 149208, 149234, 97976, 60589, 149253, 149265,
    149280, 149298, 149312, 149323, 144626, 149337, 149347, 149357, 149364, 149372, 149388, 149435, 149400, 140618, 23173, 23210,
    149171, 149465, 149482, 149497, 149515, 113497, 149532, 135799, 148063, 149583, 149259, 112287, 112293, 149598, 149608, 16467,
    149617, 149647, 149664, 149670, 149677, 130303, 130621, 130774, 133954, 133992, 149703, 6225, 149733, 149751, 149768, 149758,
    149789, 149820, 149837, 149844, 149853, 149793, 149901, 149910, 149920, 149942, 149953, 41329, 105962, 149965, 149975, 141005,
    149995, 87062, 93183, 122264, 150015, 8874, 69765, 150038, 150095, 49613, 150178, 49620, 150197, 150213, 150230, 150239,
    149932, 150250, 83384, 118310, 150219, 58281, 150263, 150282, 150292, 150304, 150321, 150366, 150383, 150337, 150352, 59855,
    150398, 150409, 150403, 124806, 142641, 150449, 150458, 150495, 150503, 150524, 150545, 103167, 136963, 41357, 150579, 150593,
    150602, 150615, 2878, 142805, 42745, 150638, 150659, 150706, 37107, 43365, 109044, 150726, 150733, 150757, 61163, 150803,
    150741, 149073, 150812, 150749, 150822, 150836, 150843, 150851, 56200, 150864, 150872, 150889, 150909, 150917, 4915, 57165,
    150925, 150947, 150958, 28996, 143625, 127782, 150983, 150996, 151011, 55321, 151034, 151043, 151053, 151073, 151091, 151136,
    151165, 151064, 151182, 151102, 151209, 151082, 151231, 151240, 151259, 151268, 151016, 143110, 151280, 151289, 67128, 106716,
    151307, 151359, 26838, 91046, 26844, 151395, 151416, 2455, 151436, 151463, 151480, 151497, 151421, 151504, 2052, 143646,
    151510, 151523, 7849, 151530, 151535, 151545, 151555, 151562, 151569, 151589, 151623, 151600, 151636, 151611, 7854, 147503,
    10956, 151650, 131975, 151666, 151657, 123124, 151707, 151723, 15562, 151763, 151787, 151804, 151831, 151729, 151850, 26606,
    150931, 151891, 151904, 150045, 151920, 151936, 151986, 151992, 152005, 152016, 108185, 152040, 152054, 108212, 152065, 152092,
    152122, 152130, 152136, 152158, 152173, 138743, 138706, 152186, 138752, 134228, 152209, 152243, 103836, 3938, 152272, 134201,
    152306, 152316, 152327, 152252, 152340, 152377, 152397, 85926, 9335, 111684, 152416, 132852, 150269, 150276, 330, 76359,
    152439, 143709, 152460, 152472, 152102, 152497, 152510, 152522, 152542, 152529, 152558, 152571, 152585, 114119, 152615, 152650,
    152625, 152667, 87200, 152683, 152695, 152706, 152717, 152739, 53801, 152755, 152564, 152769, 152786, 152800, 152816, 152834,
    152806, 152537, 143436, 152854, 152884, 152905, 152917, 102927, 127890, 102954, 127896, 152577, 152591, 152638, 152674, 152660,
    63985, 69985, 152931, 86234, 152941, 152951, 82436, 87267, 144123, 144129, 2973, 144137, 44530, 152969, 152981, 152993,
    153010, 153027, 153045, 153085, 143714, 68258, 143720, 152465, 153112, 152688, 153133, 30459, 153149, 77608, 153166, 153181,
    153199, 153252, 87149, 153265, 153278, 153307, 153330, 153257, 135773, 139372, 153367, 103049, 16267, 68077, 57423, 57429,
    153379, 153397, 153273, 153426, 153495, 153510, 153286, 153339, 153539, 110875, 153558, 153572, 153588, 153606, 153621, 153502,
    12888, 69420, 132236, 153293, 153349, 153637, 153657, 91807, 153667, 153691, 153676, 153709, 153726, 153746, 153766, 153774,
    153781, 153796, 153814, 153837, 152444, 152453, 153862, 51229, 153886, 153905, 153923, 151365, 30283, 153959, 50036, 153978,
    153992, 154018, 153983, 154045, 154062, 154102, 154138, 154068, 57903, 154156, 154162, 154168, 154192, 154206, 154225, 153373,
    147375, 154238, 40363, 154255, 131800, 154273, 154284, 154314, 151796, 151813, 151742, 151821, 132499, 154335, 78103, 78109,
    154352, 152011, 154366, 154386, 131737, 82835, 154404, 154422, 154472, 154484, 154494, 154502, 154511, 53187, 78290, 154528,
    37345, 154546, 154553, 154561, 154569, 154578, 154585, 77282, 154592, 154358, 154607, 154646, 68088, 150550, 56516, 64748,
    154664, 64765, 137762, 154678, 154688, 67133, 154704, 39282, 154716, 154729, 154740, 39295, 61918, 67142, 106841, 154789,
    154813, 154825, 154839, 154861, 154870, 154794, 99319, 154880, 154904, 154918, 63459, 63468, 153736, 137644, 150896, 152022,
    154927, 154935, 154949, 154959, 154967, 154983, 89051, 155002, 154278, 155014, 155044, 155055, 155068, 155090, 155078, 117679,
    21163, 155122, 155158, 155206, 155222, 155251, 155269, 155285, 155300, 155317, 155324, 151999, 155293, 155331, 155339, 131678,
    155369, 155389, 133380, 147507, 37452, 155411, 155421, 155436, 155469, 155494, 155513, 155521, 155428, 106780, 155532, 155564,
    155572, 155580, 155596, 155620, 155632, 155651, 137678, 2887, 155678, 155697, 141686, 155711, 62576, 155715, 155724, 29240,
    78459, 130787, 143120, 8743, 155767, 155775, 155785, 155818, 155853, 155863, 155874, 141918, 155896, 79633, 155922, 155932,
    147144, 40770, 148852, 72269, 72278, 72287, 72297, 155700, 155348, 96509, 155941, 155953, 42254, 84540, 155990, 156015,
    156023, 156030, 156038, 156047, 156066, 78221, 140864, 156103, 156126, 156160, 156169, 156194, 156203, 132783, 156219, 156237,
    156256, 156292, 156302, 156331, 156364, 25441, 156381, 91942, 156395, 156431, 18926, 156400, 156448, 156460, 156479, 156486,
    156495, 156504, 156512, 156524, 156470, 128173, 156533, 108728, 108754, 156553, 156591, 156604, 98759, 156622, 156637, 156661,
    156672, 156625, 156690, 156701, 156695, 156719, 156731, 156764, 60795, 94181, 156784, 156813, 99376, 156837, 156850, 156865,
    27071, 156914, 156945, 156963, 156979, 157038, 156970, 157056, 157069, 157080, 157089, 59662, 157101, 59671, 157110, 157121,
    157136, 157149, 157158, 157169, 157211, 157222, 157233, 122986, 54854, 9211, 157253, 157298, 157308, 157325, 157336, 157347,
    157394, 157417, 157360, 157474, 157369, 157491, 157501, 157512, 119954, 157521, 157554, 157618, 105573, 105599, 157682, 153756,
    157708, 157737, 157745, 157754, 128703, 157765, 157817, 108736, 108762, 157827, 75958, 157838, 11176, 157860, 108315, 157875,
    157902, 157921, 157942, 72445, 157965, 157980, 157993, 155948, 77429, 158020, 158033, 158044, 158039, 48304, 158065, 158078,
    114296, 158098, 6330, 158116, 158134, 158166, 158186, 158239, 158200, 158125, 158144, 158261, 158212, 158225, 158277, 100718,
    158288, 158347, 75039, 75129, 158365, 75080, 158154, 158388, 158404, 158176, 158413, 63962, 2793, 158437, 158457, 39041,
    158477, 52566, 77400, 158534, 158557, 101815, 158596, 47547, 158621, 158630, 98641, 158642, 158636, 83451, 41614, 41633,
    158625, 158661, 158652, 12527, 72737, 158676, 158694, 158725, 158737, 158687, 158744, 158705, 158714, 158751, 111143, 158769,
    111153, 158779, 126091, 156640, 158804, 155586, 158818, 158865, 626, 656, 158824, 158876, 1631, 1528, 1642, 1501,
    105214, 70632, 158894, 158909, 158922, 108681, 158937, 158951, 158964, 158979, 140225, 158991, 140235, 159011, 159058, 159025,
    159083, 131895, 159096, 159109, 159132, 159156, 159188, 159200, 159212, 159243, 159250, 159259, 22218, 126700, 159280, 159292,
    69547, 14525, 159305, 159322, 159355, 159367, 159375, 159384, 159405, 159419, 159439, 159428, 159481, 159510, 85801, 159533,
    159551, 159576, 159605, 88394, 159628, 56161, 159649, 110550, 50227, 50237, 159674, 159713, 158982, 159748, 159808, 288,
    318, 74626, 159824, 159844, 159858, 159830, 159850, 159868, 159878, 159894, 159361, 159920, 159954, 159964, 159975, 159988,
    160001, 160011, 160034, 160054, 160043, 160063, 160107, 160130, 160134, 149218, 160142, 160161, 160151, 160177, 160217, 160238,
    20059, 37899, 101620, 59021, 46652, 160261, 7858, 159837, 160276, 160293, 79745, 66978, 160301, 2936, 160323, 100729,
    160346, 160365, 1961, 160377, 44648, 160410, 160434, 21049, 51844, 52100, 160478, 104579, 160512, 134583, 160538, 160547,
    160558, 160602, 160569, 65476, 44071, 99178, 160629, 73937, 160660, 160694, 69732, 68385, 160716, 160740, 160761, 160782,
    111358, 160802, 160814, 160824, 160834, 160844, 160854, 21641, 157867, 160865, 160874, 160883, 160899, 160929, 160960, 160972,
    160981, 160988, 161004, 161033, 161046, 21647, 161077, 145144, 161092, 161101, 161112, 161122, 26941, 123271, 137462, 35609,
    161134, 161147, 161160, 161184, 13604, 161215, 161244, 161255, 161264, 161283, 161294, 161305, 161314, 161335, 161353, 161369,
    161377, 38274, 161387, 161403, 161430, 161459, 161489, 78931, 1870, 161506, 161527, 161544, 161562, 161553, 161571, 161588,
    29447, 161629, 161647, 161654, 161672, 161706, 161717, 161730, 161752, 161736, 161776, 161794, 60696, 60735, 161811, 87663,
    161825, 161857, 70665, 92419, 57492, 136263, 161891, 100249, 161909, 161923, 161944, 161964, 161999, 162015, 61957, 100518,
    162060, 162079, 162102, 162128, 162147, 56963, 162180, 125974, 162196, 126048, 126950, 162222, 162304, 162331, 162318, 162346,
    162360, 162390, 10895, 162403, 110349, 43238, 162421, 162429, 43269, 162439, 162450, 162412, 162459, 162475, 132038, 162490,
    162519, 162567, 162582, 162593, 162603, 162529, 162621, 162631, 162642, 162682, 162694, 162704, 162714, 162725, 162744, 162797,
    162819, 162837, 162875, 162887, 162899, 162757, 162922, 162940, 162967, 52680, 2520, 155133, 162997, 163007, 163035, 163067,
    25904, 163086, 163106, 25914, 163096, 163117, 163136, 163172, 12441, 133281, 133292, 163196, 161972, 163214, 163235, 163261,
    67211, 78372, 163286, 163313, 163330, 163347, 163365, 113186, 160484, 163387, 163409, 163422, 163439, 4162, 4256, 163480,
    42000, 119796, 163503, 163534, 163513, 163553, 163586, 161804, 163600, 163626, 163673, 163704, 64412, 157930, 157847, 163727,
    163765, 163782, 163801, 25811, 163833, 163853, 163874, 142945, 163895, 163914, 163950, 163969, 163993, 164013, 9828, 164099,
    161395, 37674, 25180, 157262, 164118, 164134, 164150, 61510, 164171, 164187, 163840, 57457, 6139, 164221, 160792, 164243,
    164260, 164277, 164306, 164322, 164314, 164330, 164349, 164358, 164380, 164406, 164445, 164417, 23182, 54367, 57503, 156742,
    164470, 164489, 164518, 19491, 164534, 164548, 164571, 164587, 162538, 162548, 164600, 164612, 75893, 162766, 162808, 162779,
    162931, 162949, 162958, 162788, 164626, 164651, 54378, 164660, 57515, 164694, 164195, 5497, 164672, 164765, 164783, 138505,
    164457, 164809, 164373, 164828, 164848, 164679, 164685, 163633, 29087, 120397, 164867, 161979, 164890, 121459, 164906, 164934,
    114402, 164946, 164977, 164994, 161086, 161987, 4172, 12599, 12618, 12634, 12654, 12718, 74105, 165036, 165047, 165059,
    165077, 165085, 165095, 165116, 165121, 165127, 165147, 165196, 165209, 165236, 165261, 165137, 165272, 165246, 165286, 165298,
    165291, 82082, 165310, 24638, 60091, 100740, 165326, 165336, 165346, 165354, 165390, 165419, 165481, 165491, 165503, 165514,
    165527, 165541, 128523, 128533, 128496, 165557, 165577, 165584, 29620, 138278, 11046, 11057, 165592, 165603, 13050, 165614,
    165637, 165624, 60139, 165658, 165675, 165695, 165710, 165720, 165736, 165747, 140388, 157378, 157483, 157386, 165760, 165769,
    165779, 165793, 9844, 109939, 120551, 165808, 158608, 120561, 165818, 165828, 81701, 107452, 165863, 165883, 120586, 165889,
    86943, 165896, 120598, 165914, 165930, 165949, 165923, 165967, 165991, 166006, 109802, 165782, 166022, 21209, 21185, 166044,
    166083, 166116, 166126, 158089, 158072, 166119, 166161, 166172, 166184, 89741, 166207, 166229, 24450, 166276, 24459, 166285,
    166296, 166307, 80719, 155886, 166318, 166332, 116566, 166355, 166363, 166373, 166396, 166400, 166405, 166420, 89515, 166455,
    166474, 166484, 166521, 166549, 166571, 166585, 166591, 166608, 166654, 166670, 166325, 166687, 166737, 166753, 166770, 166698,
    166804, 166817, 158906, 128616, 166830, 77251, 153001, 166855, 162466, 166870, 166877, 166894, 166911, 65748, 132022, 166928,
    166939, 166971, 37870, 166992, 167042, 167069, 72593, 167091, 167120, 167154, 167167, 167193, 167225, 167076, 167229, 41083,
    167237, 167258, 125609, 167274, 167298, 167350, 167372, 167385, 167399, 167418, 6339, 114980, 116719, 167443, 167467, 167484,
    13185, 13215, 13254, 13273, 13224, 13295, 167513, 110951, 167530, 80872, 13316, 167522, 167562, 167583, 18130, 167601,
    167618, 167637, 167645, 167671, 167681, 157626, 44295, 157563, 167691, 167726, 167733, 161783, 163417, 163448, 12661, 162498,
    7236, 7254, 79124, 167741, 7366, 167759, 167782, 167798, 167817, 12767, 90478, 167836, 167854, 167883, 167892, 4855,
    126927, 167901, 126934, 167933, 167946, 167961, 167972, 167985, 160907, 167804, 163206, 166711, 167789, 7504, 61737, 61746,
    13348, 13358, 13382, 77308, 168004, 168020, 168032, 168043, 168055, 168064, 168075, 168091, 168111, 168133, 168143, 168153,
    168170, 168162, 168181, 168191, 168200, 168210, 168101, 168122, 168223, 162027, 166340, 88025, 168243, 168252, 168263, 168274,
    72217, 72225, 168284, 168293, 114872, 168304, 168330, 168343, 168023, 168316, 37970, 168369, 88957, 59027, 31506, 168389,
    168405, 168430, 168417, 168471, 40492, 166057, 168556, 168567, 168577, 168600, 168613, 168627, 114303, 114377, 12669, 130393,
    168650, 109408, 30535, 145036, 168707, 168718, 168730, 168744, 168775, 168788, 168735, 168804, 44244, 168822, 159887, 168620,
    168872, 28789, 156630, 156644, 156664, 14791, 6748, 168885, 168902, 168922, 168938, 168949, 163682, 168959, 159757, 163905,
    168991, 169007, 169026, 169050, 165067, 14804, 169069, 158810, 156652, 142653, 142673, 142760, 169111, 169164, 169203, 167202,
    169121, 169231, 169267, 169301, 169367, 169411, 169238, 91082, 169451, 116938, 169518, 169581, 169603, 169620, 169658, 169633,
    169678, 169709, 169687, 169698, 8694, 35619, 136491, 161323, 158943, 169728, 169755, 101492, 92293, 169770, 76073, 169780,
    169788, 15618, 4059, 169798, 143046, 169840, 30043, 169868, 3033, 131002, 138350, 169894, 10434, 169942, 169951, 10684,
    160613, 169962, 169978, 170004, 170022, 170046, 170065, 170079, 170111, 152843, 170127, 170162, 170196, 170217, 170247, 170230,
    170269, 159267, 170291, 170316, 170328, 101241, 168894, 170360, 138460, 170430, 120021, 8924, 170453, 170471, 170462, 170496,
    170514, 165318, 157317, 170525, 170534, 141944, 170544, 170553, 4070, 4104, 170563, 170589, 170616, 170681, 170699, 170709,
    170690, 170718, 170729, 170740, 170750, 170761, 170769, 170802, 170780, 170813, 34474, 170791, 170824, 170835, 170845, 170862,
    158570, 170882, 158584, 170896, 170436, 170442, 28041, 161834, 170908, 170920, 170933, 170954, 170976, 170965, 139636, 139647,
    170999, 144856, 171020, 171032, 170483, 171042, 9731, 171069, 171086, 171096, 171119, 137747, 171145, 171164, 171177, 171201,
    171238, 171190, 171253, 171265, 12002, 171277, 12346, 171296, 171303, 171315, 171330, 171348, 171370, 171359, 171390, 171380,
    171428, 171444, 171154, 171460, 37913, 171500, 171514, 132735, 171530, 169588, 69655, 130429, 171595, 171610, 63717, 63728,
    171630, 171645, 171657, 171707, 171721, 171743, 160187, 171765, 171784, 171754, 171822, 171833, 26204, 160228, 171843, 61970,
    171774, 171878, 171902, 26231, 166137, 171926, 171946, 171969, 171993, 172024, 172042, 38163, 171915, 138887, 172054, 170627,
    61329, 61341, 171436, 172079, 172096, 172108, 139185, 91296, 11571, 165566, 172120, 11587, 171935, 172140, 172159, 172204,
    172225, 172007, 172249, 172275, 172262, 169377, 172311, 172355, 172381, 172407, 172477, 172417, 172429, 172497, 172514, 172539,
    30922, 172505, 146218, 172563, 172368, 172394, 172442, 172487, 172452, 172577, 172589, 172464, 76085, 156753, 156774, 169389,
    172622, 172658, 172666, 172676, 172685, 91192, 172743, 172760, 172772, 74514, 172780, 172787, 172804, 172840, 172855, 99901,
    172880, 124258, 172848, 172907, 172927, 59052, 172943, 172956, 83393, 107745, 172972, 12802, 12808, 172999, 173016, 173008,
    173033, 173051, 173064, 173085, 103648, 173102, 173119, 173159, 173055, 173189, 173238, 37924, 66165, 173262, 48749, 173272,
    160247, 173285, 173297, 173319, 101140, 173338, 43810, 173352, 148072, 14615, 100818, 149591, 173366, 8940, 173391, 173402,
    159393, 173414, 173422, 41234, 62590, 173428, 117729, 65004, 173441, 173471, 173448, 99726, 173486, 170089, 173490, 173549,
    173562, 173497, 173556, 173572, 173581, 147617, 173606, 173625, 173664, 173683, 143567, 173700, 173710, 53233, 63550, 166013,
    173454, 173503, 173729, 173749, 173771, 173826, 50921, 173873, 173944, 173974, 173993, 173878, 174000, 174019, 174035, 174056,
    174064, 174074, 174090, 174153, 134211, 139150, 174170, 133654, 174185, 48459, 174204, 174246, 153718, 174271, 174280, 174290,
    173840, 174306, 112487, 76840, 174339, 90323, 174352, 38808, 116824, 10334, 174416, 88061, 174444, 10347, 174430, 174465,
    167700, 174487, 90329, 140847, 174505, 174516, 174552, 174524, 173130, 164481, 174566, 174594, 160772, 174612, 174637, 174655,
    169174, 30620, 174672, 27938, 174689, 174711, 27948, 174699, 174798, 174006, 174012, 174816, 174847, 174863, 23341, 174893,
    174944, 174969, 175005, 15766, 175024, 175067, 175077, 174828, 173479, 175087, 175106, 175181, 175116, 175203, 175225, 175241,
    175253, 131930, 173278, 175300, 175309, 175320, 4267, 57526, 175397, 175405, 175415, 175445, 133125, 175465, 175477, 175491,
    175514, 78875, 175525, 175539, 175548, 168911, 175574, 175619, 175644, 44367, 175664, 175674, 175702, 80454, 175712, 140450,
    175726, 175735, 140073, 150990, 80460, 175718, 175746, 70116, 63476, 175776, 175807, 175818, 175827, 175839, 170096, 175870,
    175877, 175885, 44371, 78639, 10067, 78645, 175668, 4630, 37607, 4638, 175900, 174161, 71813, 136930, 129439, 84967,
    71819, 175914, 175958, 134739, 175986, 25703, 176003, 176009, 176017, 176023, 173949, 20522, 176031, 11861, 176044, 176065,
    11899, 176103, 131743, 176111, 176154, 176037, 11871, 176054, 176075, 11913, 131752, 176120, 176163, 121657, 154922, 67790,
    67837, 67889, 173362, 173460, 68565, 176173, 176187, 18651, 121527, 144146, 176195, 176213, 41741, 176237, 144154, 176255,
    169905, 176267, 48637, 176280, 16473, 176302, 134100, 152219, 176327, 176351, 176312, 176363, 176372, 28733, 176390, 173435,
    117739, 65011, 176407, 176422, 176446, 176469, 176473, 176481, 176495, 176504, 176514, 176532, 133231, 176562, 176574, 46170,
    176591, 121256, 176610, 176625, 14020, 59893, 156597, 176641, 176653, 27324, 49227, 176666, 176677, 176697, 174027, 176718,
    176736, 176753, 176770, 176785, 176794, 176800, 49056, 176807, 176817, 62978, 176829, 62984, 174211, 176861, 176868, 123926,
    123959, 176745, 148358, 176874, 176898, 176885, 176915, 176937, 176953, 176977, 176988, 176961, 177005, 177033, 177047, 177064,
    177039, 121619, 177096, 177108, 177127, 177009, 177160, 177177, 177195, 177214, 177220, 163924, 176999, 177227, 177238, 177251,
    177245, 177260, 177266, 74943, 177291, 177356, 177363, 107666, 177371, 177377, 175678, 176429, 177387, 177404, 177426, 177415,
    34263, 177466, 34271, 177474, 177481, 177489, 177494, 177500, 177521, 177507, 177570, 177578, 177589, 73316, 177602, 176523,
    177626, 177659, 177702, 177633, 177756, 177781, 177802, 177816, 177831, 177849, 177860, 177871, 177884, 177891, 177903, 176398,
    177935, 177953, 177944, 159864, 177969, 177485, 177981, 173170, 94206, 177994, 178006, 178021, 178043, 178065, 94212, 178000,
    178032, 178054, 178076, 178087, 178110, 178131, 178153, 178168, 178141, 176672, 166492, 178202, 173850, 178218, 178243, 173138,
    38963, 173148, 38974, 142073, 178259, 178280, 178294, 178311, 178328, 178346, 173762, 29031, 178426, 178435, 16451, 51721,
    93022, 178445, 178467, 178485, 175014, 178475, 130260, 137103, 174560, 137180, 66051, 69719, 177276, 178504, 177300, 34278,
    178536, 178550, 481, 512, 1233, 1155, 1144, 110160, 51884, 93037, 8758, 178564, 178572, 178583, 178605, 80882,
    178631, 126797, 178650, 178094, 178678, 178693, 178706, 17831, 17839, 104970, 105080, 178746, 24374, 178759, 178770, 178783,
    178806, 178816, 178718, 178659, 178828, 178848, 178668, 178861, 178883, 178752, 178901, 178918, 178934, 178853, 178954, 105124,
    153315, 178972, 178995, 125675, 125694, 179021, 179034, 178979, 178687, 100108, 179047, 179059, 179077, 179089, 179106, 111558,
    179123, 111601, 179133, 179150, 145745, 179169, 117354, 179097, 98167, 130413, 179187, 179208, 136177, 179226, 143923, 142834,
    176646, 179249, 101928, 142841, 179262, 142903, 179286, 179297, 179275, 120227, 179309, 179317, 104873, 140053, 179348, 103547,
    104880, 69125, 69136, 82897, 179364, 177167, 179378, 66252, 179390, 179410, 179428, 179445, 71138, 179465, 179513, 36205,
    48387, 145191, 24578, 152776, 179533, 179523, 179553, 179545, 179572, 179589, 179604, 179621, 179638, 179667, 179683, 179714,
    179730, 161597, 179751, 179792, 179811, 159312, 179143, 25363, 179831, 179843, 179860, 179850, 179873, 179889, 179920, 179931,
    25451, 179940, 179977, 179985, 71897, 93932, 102212, 179997, 180013, 180027, 180036, 180046, 180056, 180067, 180098, 180117,
    180134, 180155, 180175, 31356, 133716, 180192, 180163, 100112, 180205, 100117, 175521, 180224, 180242, 180255, 175583, 180267,
    180292, 180314, 175558, 180339, 180358, 175593, 180376, 175600, 180410, 179897, 180426, 180434, 179384, 180440, 154751, 180451,
    180482, 180489, 180462, 27878, 180497, 153643, 180508, 180523, 180533, 180557, 180589, 180627, 180568, 180646, 180664, 153700,
    40849, 180682, 180701, 126101, 80476, 180710, 180513, 180720, 180741, 180758, 131900, 180771, 27884, 180503, 60218, 156794,
    180783, 180794, 60805, 180810, 60812, 180817, 180823, 42146, 180866, 180891, 47757, 178542, 180921, 180949, 180971, 180982,
    181006, 181032, 181043, 51650, 51659, 181037, 180777, 14146, 131823, 181062, 181098, 181109, 181068, 63200, 170261, 181078,
    71212, 181123, 181140, 181159, 181191, 181197, 181206, 49426, 181222, 181233, 181214, 181246, 181265, 181284, 181330, 181380,
    181399, 11321, 78470, 87006, 106913, 181417, 181440, 181457, 181516, 181466, 61762, 181596, 181526, 181536, 181670, 181476,
    181547, 181486, 61773, 181611, 181558, 181569, 181685, 62832, 181712, 181730, 181749, 181762, 181779, 181798, 181816, 60224,
    63360, 131452, 151673, 181836, 181851, 181865, 164702, 181894, 81657, 164713, 181905, 181915, 181928, 181945, 181957, 74862,
    181977, 131851, 182000, 182012, 165223, 24200, 182053, 118795, 25723, 182064, 182082, 135497, 129168, 182097, 182122, 110844,
    159164, 182161, 182178, 182109, 182170, 124041, 102501, 182194, 182207, 14185, 13004, 182244, 182256, 32921, 182274, 182305,
    182337, 182359, 182394, 182415, 182432, 124159, 182452, 182461, 182470, 179562, 182476, 179051, 117814, 117866, 131857, 182215,
    64113, 182507, 182518, 103057, 16276, 77368, 182539, 182544, 182553, 104206, 83296, 182577, 182624, 19569, 136696, 136724,
    182641, 14209, 182669, 182647, 182699, 182712, 106846, 182721, 182736, 182727, 182755, 182778, 182786, 154515, 151579, 147105,
    182830, 182842, 179612, 182858, 42157, 182874, 180443, 180835, 180845, 182905, 152258, 182529, 182927, 182942, 182965, 71157,
    67382, 182982, 121261, 174175, 183012, 63949, 182849, 182865, 183022, 183046, 12906, 183061, 183087, 183108, 183119, 36527,
    160254, 46703, 152232, 152265, 183131, 183142, 183161, 183151, 183170, 152549, 183113, 183211, 93215, 93222, 183227, 183239,
    183274, 183297, 183319, 183305, 183327, 183341, 183366, 183378, 183347, 183411, 183439, 183356, 183466, 130701, 26816, 14046,
    168561, 124106, 183492, 183505, 183526, 183554, 183560, 181449, 131862, 182223, 183568, 183586, 183017, 182282, 183605, 183622,
    13010, 182250, 175470, 183646, 48677, 183663, 183671, 177235, 183690, 117424, 75093, 183701, 75101, 183709, 183724, 183732,
    163003, 163013, 183741, 163017, 177595, 177665, 177768, 177791, 177877, 150020, 183752, 64826, 62651, 74780, 132397, 183759,
    150026, 183765, 183747, 183786, 183816, 183831, 134637, 183849, 183857, 124049, 183864, 183875, 183900, 183909, 174099, 183918,
    183926, 183954, 183982, 184000, 183937, 183965, 184021, 183973, 184029, 183990, 183946, 184038, 184052, 184067, 184092, 15246,
    184121, 184136, 184142, 10963, 13625, 184151, 86218, 184164, 13656, 184179, 184185, 48798, 184192, 184201, 184212, 165364,
    184226, 99259, 184244, 184256, 184289, 184325, 26849, 184098, 184355, 184375, 184440, 184448, 184458, 184467, 184491, 184516,
    73018, 184534, 184543, 178355, 184551, 184569, 184584, 184601, 184635, 184661, 184693, 184719, 184610, 184745, 184525, 53192,
    131213, 131241, 156560, 184765, 184783, 184771, 184798, 184815, 184848, 184871, 47317, 151925, 115045, 161344, 184927, 174853,
    184948, 125261, 69616, 184806, 175754, 184966, 184991, 185038, 185054, 78978, 185071, 185080, 74539, 172151, 185091, 185107,
    185138, 127622, 185157, 185176, 185205, 96430, 108105, 185222, 185241, 185279, 185294, 11612, 185315, 175127, 185325, 175139,
    185341, 185358, 14084, 185375, 32034, 33218, 185214, 185166, 112521, 112529, 58389, 185409, 185422, 120718, 120674, 185440,
    185469, 185489, 169850, 169876, 4116, 185511, 147807, 125358, 185534, 58399, 185549, 116744, 121381, 185567, 185589, 185605,
    185639, 185616, 185661, 185628, 185650, 185671, 168589, 185682, 156053, 166618, 185708, 27551, 185729, 185753, 22426, 185598,
    185786, 97230, 185801, 11472, 11494, 185825, 185838, 185852, 90159, 185866, 185877, 31849, 172601, 64689, 83177, 1027,
    185911, 51143, 185830, 60755, 185843, 89456, 185934, 93121, 185949, 985, 1006, 38409, 11805, 77052, 155963, 17644,
    185962, 150031, 185973, 186004, 185984, 186045, 14336, 54141, 180073, 177712, 186062, 180082, 184380, 54174, 71794, 186081,
    186100, 186106, 186115, 186128, 186163, 186187, 186271, 186279, 186298, 1508, 186192, 432, 467, 1112, 186342, 59877,
    186366, 150002, 121544, 142885, 122270, 27201, 186379, 122554, 186391, 186425, 71828, 186473, 136195, 182366, 71850, 186501,
    186522, 186533, 145493, 69372, 69382, 136203, 174255, 186545, 173109, 163251, 186382, 186558, 186569, 71860, 71873, 186581,
    186592, 64700, 186604, 186655, 186670, 186680, 186610, 186690, 83192, 186717, 136499, 186741, 186777, 59691, 74499, 173884,
    133632, 133665, 186788, 149242, 92309, 186805, 186810, 9472, 186818, 60818, 84615, 2859, 60827, 186844, 186860, 139889,
    186893, 186902, 186913, 58543, 186922, 56286, 186933, 186966, 186943, 186986, 187004, 171541, 86000, 186994, 187020, 134516,
    86279, 68694, 187033, 187043, 187059, 187073, 187083, 186749, 186758, 187095, 187110, 187125, 187143, 164723, 187151, 95396,
    187160, 92314, 98201, 29039, 187178, 187209, 187224, 187264, 187293, 187304, 187310, 187298, 140572, 125819, 125838, 187317,
    115785, 187333, 8114, 10525, 19591, 187350, 187364, 187383, 97345, 187402, 187420, 187426, 187433, 187471, 144163, 40889,
    176203, 187489, 34189, 106797, 187519, 96831, 106754, 187559, 96841, 106764, 187569, 187580, 187587, 187595, 187606, 174108,
    182950, 187616, 187627, 187635, 48344, 187650, 187666, 187683, 187701, 187719, 46709, 89831, 187735, 187774, 46718, 187791,
    187801, 122714, 122720, 187815, 187823, 29137, 187842, 149304, 89837, 108137, 108146, 187746, 51739, 178616, 185288, 187852,
    173245, 173254, 187867, 46227, 187875, 185231, 92955, 187027, 187884, 187910, 187920, 187941, 187960, 187970, 187526, 187980,
    188001, 183695, 75053, 183715, 75107, 75142, 188019, 188053, 188073, 146834, 188080, 180248, 187533, 188096, 188110, 157180,
    188138, 188148, 188159, 122107, 176456, 67746, 34885, 152142, 152151, 60114, 134644, 188171, 188183, 188205, 188215, 96562,
    187389, 188235, 184445, 175834, 48308, 47765, 64652, 165157, 188250, 188264, 181983, 182021, 188273, 188278, 188289, 176837,
    176842, 176848, 176855, 62512, 15357, 132465, 132475, 188298, 188327, 78748, 188364, 188432, 64603, 188454, 64659, 77225,
    188467, 188478, 188496, 170073, 188487, 188505, 188514, 188523, 188539, 188556, 188582, 188629, 188666, 184391, 85753, 188687,
    188708, 188743, 188758, 188775, 188795, 169467, 28380, 28359, 188810, 188802, 188818, 188829, 188837, 124166, 188847, 188859,
    177672, 23242, 127566, 179946, 188879, 188888, 188894, 182956, 77843, 185557, 188910, 188919, 188901, 25188, 25534, 177283,
    177311, 188934, 165939, 188948, 188969, 188986, 103013, 92930, 124311, 188999, 189017, 189032, 188993, 175233, 175262, 175274,
    46071, 189048, 189092, 166350, 177323, 189128, 94253, 168083, 189145, 189159, 20690, 189164, 189196, 189208, 78385, 121904,
    179325, 126779, 188061, 189234, 67756, 189278, 189327, 174680, 189350, 189359, 189170, 189175, 189202, 76310, 189370, 189389,
    64836, 62657, 189396, 189402, 189413, 189424, 189433, 155443, 180990, 94791, 155453, 189454, 189460, 189466, 189480, 189489,
    63014, 31157, 189521, 189542, 189556, 90706, 28822, 96438, 98206, 189569, 123130, 149983, 151312, 189580, 189591, 189598,
    189606, 189561, 9504, 23034, 41053, 189616, 189628, 189648, 189703, 189721, 81345, 189733, 39592, 78136, 189744, 54089,
    54095, 189763, 189777, 189787, 189753, 58816, 95406, 181739, 97630, 189796, 189858, 92330, 189877, 189893, 79476, 189918,
    187066, 84354, 189930, 189405, 189470, 113203, 189737, 171714, 189949, 58831, 189964, 3262, 34227, 159448, 189981, 189998,
    190018, 190040, 159457, 190008, 190029, 190051, 159469, 190071, 89844, 124457, 190091, 190096, 190141, 190157, 190200, 190216,
    190253, 190271, 74054, 190102, 190147, 190163, 190296, 190327, 190343, 190305, 190312, 190319, 190368, 190396, 190418, 190405,
    190427, 190378, 190446, 190488, 190509, 16792, 190388, 83521, 190526, 190349, 190552, 190559, 190567, 190579, 190605, 190622,
    190646, 102659, 190661, 190679, 14488, 59101, 59130, 190714, 41401, 190735, 155502, 96364, 30467, 112805, 190757, 190774,
    59105, 159725, 190794, 190807, 190824, 71223, 190853, 94076, 16180, 30474, 103684, 107372, 190874, 190889, 190914, 59113,
    41382, 185857, 190897, 190922, 98303, 190905, 190930, 98312, 190945, 94797, 190955, 5259, 191008, 164899, 191037, 191052,
    191061, 191069, 191080, 191134, 191012, 191021, 87791, 106923, 191153, 191180, 191209, 191232, 191260, 191167, 191195, 165371,
    191280, 191292, 191306, 191221, 191325, 122820, 151110, 107379, 191333, 191350, 107480, 6715, 191340, 19078, 191369, 125732,
    191397, 48466, 71234, 190864, 181166, 191417, 68066, 189585, 187847, 189925, 191432, 79755, 190613, 66989, 190653, 191450,
    75559, 191474, 105130, 103109, 191496, 104226, 191519, 191532, 191553, 52205, 191572, 189553, 191585, 131338, 189566, 191594,
    191607, 31955, 106163, 191620, 191625, 191649, 191659, 191633, 191677, 156800, 120737, 191685, 191689, 191697, 191714, 191705,
    191728, 191740, 191761, 160358, 34196, 96785, 191802, 191812, 191818, 191837, 191847, 191858, 191871, 191889, 191903, 191915,
    52382, 191928, 191948, 191956, 191822, 191965, 100825, 191986, 191997, 119965, 192010, 192022, 192034, 192070, 42676, 192092,
    192103, 174361, 192116, 192134, 192124, 192142, 192158, 192177, 82008, 192165, 192172, 13497, 192193, 192205, 13463, 192152,
    149684, 192219, 192242, 125370, 125380, 192259, 192279, 70641, 192296, 192309, 192326, 192336, 192302, 192317, 189712, 62991,
    190332, 190724, 101103, 72473, 192348, 192360, 72485, 192390, 192406, 192420, 192432, 192455, 130910, 192484, 192494, 8955,
    157061, 192511, 42646, 192529, 192538, 192549, 192558, 192569, 192582, 192591, 192602, 192624, 192634, 192611, 192646, 192661,
    192715, 189416, 192735, 192754, 192776, 192787, 192798, 192805, 192814, 192834, 192859, 75783, 192886, 191597, 140576, 46310,
    28505, 192904, 192844, 192872, 192933, 192943, 192952, 192961, 192972, 192991, 103180, 193012, 193051, 193020, 193066, 48471,
    193087, 193096, 193104, 193112, 94483, 193156, 190337, 193188, 193203, 193221, 30684, 158053, 30694, 193209, 193240, 193258,
    191277, 193274, 54704, 54744, 159736, 193285, 193314, 193333, 189957, 193355, 76095, 124893, 193363, 76105, 138913, 193371,
    193337, 193379, 193385, 193392, 81816, 193400, 16488, 149623, 193420, 179904, 191577, 193346, 193436, 193323, 193453, 193277,
    42172, 136756, 193470, 193506, 193518, 70995, 193531, 71011, 193196, 86511, 42179, 193526, 174114, 174123, 41961, 193544,
    124462, 193561, 192225, 193577, 193595, 193610, 193584, 193602, 193631, 193643, 193650, 193657, 193671, 48838, 130335, 65925,
    193683, 193695, 193717, 82377, 193737, 193747, 161918, 193757, 99954, 119417, 132651, 193762, 132126, 178867, 193769, 193774,
    193785, 193807, 193816, 99939, 31164, 76747, 100882, 71802, 193826, 193850, 32090, 193861, 57171, 58554, 193875, 193880,
    31170, 193886, 193906, 83234, 34202, 187540, 52602, 193926, 193950, 170281, 102133, 193961, 193931, 193940, 193984, 194006,
    194021, 194013, 194031, 194042, 62480, 194066, 62520, 126110, 62528, 92655, 194086, 194101, 194116, 172815, 194092, 194131,
    194153, 194172, 194182, 194192, 49482, 194219, 194238, 194252, 194124, 194245, 194278, 194204, 194211, 194291, 15286, 194297,
    194320, 159219, 194339, 194354, 194363, 194382, 15364, 194399, 194419, 194465, 194427, 15373, 194435, 15384, 15395, 71244,
    194513, 194565, 47583, 156225, 64163, 194598, 194520, 194632, 194528, 194642, 194701, 194709, 194720, 194751, 194729, 138764,
    143727, 194738, 83787, 42111, 194785, 194815, 42120, 194859, 77859, 194891, 101573, 194907, 77865, 81117, 193477, 194476,
    194930, 194943, 194306, 194954, 194976, 194995, 151941, 194959, 52261, 195013, 194409, 195034, 195052, 195063, 195076, 106279,
    106289, 151951, 195139, 195163, 151403, 195169, 191776, 195082, 191788, 195177, 195194, 195236, 141024, 9173, 195254, 195263,
    195282, 195302, 195313, 195273, 195292, 195328, 195340, 143838, 167752, 195352, 195383, 195406, 195421, 195432, 195440, 195470,
    195484, 195495, 195508, 155626, 195519, 195534, 153565, 140060, 195544, 195551, 195593, 195562, 84719, 195630, 195656, 84736,
    80331, 195675, 195707, 92627, 195719, 195740, 135808, 195753, 61644, 61651, 130985, 61659, 195762, 19635, 195780, 195799,
    195823, 138063, 195849, 195864, 195894, 195903, 195912, 191425, 195929, 21066, 41557, 42184, 193513, 78683, 195943, 195962,
    195983, 195993, 43646, 104117, 196004, 81953, 54710, 196020, 196029, 34234, 196039, 196058, 196067, 196076, 196103, 196090,
    196117, 85089, 196130, 196140, 196151, 55354, 190355, 190361, 60834, 196163, 156340, 196210, 196218, 196273, 130446, 196288,
    70124, 75730, 70129, 75735, 196299, 196167, 103114, 196329, 196365, 196372, 196379, 196172, 196385, 70138, 19158, 26857,
    26867, 26874, 195936, 196405, 196423, 196442, 135265, 167130, 196453, 196468, 196485, 196496, 195857, 196508, 196527, 196514,
    196521, 196533, 191441, 189134, 63810, 78554, 84399, 168441, 168450, 105095, 105136, 167845, 196539, 196546, 196555, 94841,
    195874, 49010, 195883, 196604, 195921, 137850, 196630, 97433, 196641, 196655, 80362, 195202, 195211, 165382, 116506, 196670,
    196689, 196723, 196733, 192356, 44030, 196282, 196745, 196761, 159657, 191376, 196778, 44740, 196799, 18751, 196828, 196857,
    196751, 196842, 56377, 121359, 56354, 20427, 196873, 196884, 196896, 135713, 196878, 196916, 196941, 196956, 192443, 196967,
    20432, 196979, 81205, 196906, 197001, 197051, 197013, 197089, 197098, 163395, 197107, 197124, 87604, 91673, 197139, 197153,
    104748, 197167, 174905, 195409, 197189, 197202, 197214, 197195, 197232, 197256, 197266, 195414, 197278, 197333, 197353, 190881,
    197285, 197293, 197302, 73100, 197403, 197313, 197447, 197479, 197533, 197358, 197366, 73109, 164026, 197578, 197325, 174976,
    197625, 182483, 197637, 197378, 197386, 193867, 18148, 98477, 174912, 104759, 174921, 197661, 197677, 197667, 197683, 197693,
    197706, 197724, 197714, 197757, 197775, 101348, 197783, 197791, 197800, 197700, 31624, 44037, 151317, 156567, 197825, 197842,
    197861, 197875, 197887, 197905, 163563, 197867, 197932, 110510, 110517, 197972, 113278, 197940, 197992, 163455, 198016, 198043,
    198102, 193246, 198126, 90758, 161844, 198157, 198179, 142988, 198203, 198223, 198237, 198053, 198215, 198249, 198283, 198301,
    198324, 198355, 198371, 198392, 198408, 74664, 198431, 198445, 130341, 198456, 198475, 198496, 198531, 198543, 84469, 161851,
    198576, 198594, 198621, 198651, 164036, 198676, 198702, 122090, 198717, 159981, 198740, 198775, 44046, 44304, 198814, 198725,
    198830, 198844, 72714, 198863, 126535, 194142, 198880, 198902, 198921, 198948, 97993, 198967, 198854, 155732, 198993, 199018,
    74630, 199037, 198379, 198417, 199055, 198292, 198310, 199079, 55392, 198334, 154671, 198363, 198384, 198400, 198422, 199105,
    51668, 199110, 199119, 56655, 40860, 131263, 199153, 199189, 199206, 199214, 97440, 198823, 199230, 199253, 199236, 199269,
    49171, 73986, 199259, 199243, 199291, 54275, 37990, 193124, 54301, 122436, 155657, 197881, 66527, 199310, 199323, 3458,
    69773, 4279, 188441, 193164, 87287, 87309, 45699, 199337, 199342, 199351, 174931, 190107, 199060, 199361, 184236, 199379,
    132403, 72003, 199394, 124265, 198912, 72954, 199407, 93756, 189006, 199448, 199465, 188461, 199480, 199489, 104700, 199499,
    27960, 199511, 199531, 199545, 199558, 199504, 99406, 199570, 121910, 95320, 199583, 199627, 199666, 199686, 50320, 199724,
    156347, 127943, 199699, 199713, 184401, 199748, 199763, 199777, 199754, 199792, 121662, 67797, 124214, 67806, 124224, 15905,
    11337, 11343, 11357, 72009, 199825, 199832, 98764, 54307, 199840, 34346, 34355, 193171, 193133, 126078, 199866, 3640,
    199885, 199916, 55606, 199931, 124273, 104536, 104544, 99413, 116248, 121918, 199950, 54314, 199989, 122448, 199222, 192293,
    200014, 200022, 200039, 200046, 200017, 200055, 200073, 200092, 83921, 137973, 200112, 200149, 190765, 200162, 134901, 200181,
    200190, 200201, 196177, 195745, 200211, 200231, 200240, 102530, 200260, 78152, 144236, 182912, 122243, 122275, 200274, 200296,
    130352, 200221, 200329, 200361, 200377, 200369, 200386, 183233, 182920, 145447, 200395, 99264, 200423, 167136, 123543, 181968,
    200436, 196663, 193639, 195039, 200451, 200463, 200474, 106298, 195092, 200481, 45704, 163045, 200487, 34241, 141780, 200508,
    200455, 5263, 36777, 200534, 200545, 200601, 196182, 200538, 200549, 200638, 36813, 77730, 200653, 200556, 200645, 200670,
    200679, 77409, 158543, 200702, 158551, 200718, 200731, 62121, 200745, 200755, 200767, 200777, 69685, 88943, 200789, 45713,
    200807, 200828, 200841, 104261, 124649, 70089, 200443, 200855, 200872, 200886, 200890, 8279, 200900, 200923, 200936, 200953,
    200994, 24646, 201021, 201037, 200968, 200980, 201008, 200861, 65931, 193689, 193701, 193727, 106474, 201054, 106428, 192824,
    201071, 43498, 201076, 201086, 201101, 117213, 200867, 201116, 103717, 103738, 30484, 201126, 201153, 201139, 137686, 2896,
    6870, 180418, 75912, 125179, 79186, 98550, 153893, 201175, 201189, 201206, 35719, 201227, 153898, 201244, 201266, 47973,
    201287, 16154, 201314, 11144, 201331, 201341, 201353, 201390, 201421, 201428, 201451, 201481, 201502, 201521, 201544, 194605,
    58993, 201623, 201664, 201557, 201691, 201704, 201460, 201358, 201717, 166783, 201743, 62732, 62743, 201764, 201572, 201787,
    201812, 201837, 201862, 201881, 201892, 55027, 201903, 35727, 201907, 14649, 79761, 86240, 201915, 201928, 201942, 201954,
    132507, 201974, 201985, 201980, 189375, 201996, 202011, 202037, 202048, 202001, 106373, 202070, 202109, 202130, 84435, 202149,
    202016, 179235, 106380, 202082, 128585, 202116, 202168, 201320, 201325, 202191, 202198, 17664, 202204, 17541, 202222, 202241,
    87205, 202260, 202271, 202284, 202311, 202335, 202357, 170057, 27207, 151431, 202373, 202386, 92735, 202415, 156074, 202432,
    136534, 202378, 202448, 26615, 42126, 202459, 202470, 202483, 202248, 202497, 202506, 202516, 202529, 35890, 124468, 47109,
    201199, 202544, 47116, 124473, 202424, 190260, 53905, 202556, 202564, 202571, 202586, 202600, 202615, 202651, 202661, 66624,
    202671, 202691, 202716, 50710, 202752, 87220, 202777, 126809, 126840, 202817, 180005, 202839, 202858, 202874, 144442, 202787,
    5611, 202798, 202889, 202867, 202922, 202942, 104439, 76908, 126818, 202954, 202963, 47917, 202764, 202974, 58561, 202996,
    202983, 202808, 102648, 174180, 16424, 179419, 203018, 200303, 15475, 203036, 203064, 203027, 202902, 202909, 202916, 203082,
    203091, 164269, 203099, 203110, 203145, 203190, 203250, 203258, 176764, 193955, 203278, 203301, 26371, 203318, 203335, 203350,
    203379, 203342, 203357, 203389, 203400, 203440, 203409, 97271, 203420, 203430, 203449, 164874, 203459, 103470, 203495, 203513,
    203544, 203566, 66192, 66202, 203601, 81553, 203632, 203732, 81223, 37999, 203752, 203770, 203793, 41750, 203835, 203863,
    64480, 203875, 203898, 203905, 180674, 203927, 51945, 203947, 203973, 114771, 204009, 204030, 114780, 204045, 114746, 204082,
    114791, 204105, 204127, 114802, 204116, 204097, 204143, 204040, 204158, 204165, 204173, 204184, 204195, 90721, 204205, 35734,
    26380, 203327, 15631, 3476, 104650, 3513, 125781, 203641, 204225, 204245, 194326, 204255, 204272, 204282, 204287, 204296,
    94644, 94676, 94650, 94684, 204304, 204315, 204343, 204391, 204349, 204426, 204441, 93965, 125187, 204493, 204499, 125215,
    201251, 204518, 204533, 125193, 204550, 139759, 204575, 204619, 104899, 204628, 79057, 36045, 146048, 87210, 204652, 204659,
    202453, 187119, 203044, 204667, 204679, 204708, 204728, 55845, 204734, 204741, 204750, 36057, 203365, 204763, 45882, 204775,
    204788, 204798, 204815, 174083, 204834, 204873, 204889, 204914, 204824, 141788, 204881, 204932, 204938, 204948, 204779, 35276,
    204958, 204974, 204982, 204993, 205001, 76960, 202174, 22196, 83987, 93129, 205008, 205024, 104978, 205030, 205039, 62663,
    74786, 201729, 205096, 205125, 84061, 205135, 117624, 205154, 205168, 205187, 205145, 16251, 189885, 205209, 185185, 70791,
    205218, 205228, 204432, 205178, 96416, 205243, 205260, 156178, 205284, 205304, 205320, 102082, 205251, 186176, 205294, 198137,
    205336, 188027, 84931, 102091, 184335, 205355, 205374, 127449, 205395, 20121, 205412, 180182, 196225, 205431, 205463, 78086,
    205482, 41900, 41168, 156211, 91196, 205493, 205518, 205536, 199162, 74025, 205549, 140872, 179631, 179676, 155794, 205572,
    203556, 199176, 74037, 139952, 155259, 205600, 205613, 205631, 205644, 205657, 205677, 205688, 205702, 167908, 192368, 167916,
    74410, 205708, 205636, 59203, 205718, 157713, 90506, 161607, 205729, 77542, 205748, 205772, 205792, 205814, 54750, 193291,
    205839, 205850, 205869, 205887, 205895, 205905, 205912, 201259, 204526, 46492, 177962, 205933, 205946, 205967, 205978, 205992,
    206003, 206017, 206032, 166095, 206023, 121483, 206051, 165400, 206070, 165408, 12216, 41194, 86078, 206085, 206109, 206127,
    206146, 206199, 61196, 206234, 80586, 201991, 205724, 206244, 71572, 206249, 71614, 196233, 20286, 206297, 206312, 206328,
    23158, 206341, 206349, 206358, 180693, 40971, 206368, 206391, 206406, 206058, 206437, 206441, 82704, 206446, 206453, 206462,
    206483, 106513, 66455, 98651, 198257, 206504, 63826, 205957, 205128, 106486, 206520, 204211, 198836, 198731, 206542, 206551,
    206563, 140631, 202277, 206577, 131540, 149775, 206525, 91656, 140638, 53085, 137189, 206605, 206628, 27426, 27478, 206650,
    206677, 16104, 38739, 43874, 38744, 56132, 206696, 206719, 206743, 206766, 206800, 206809, 206818, 206830, 201897, 206839,
    201406, 201437, 201443, 201467, 201367, 201492, 201512, 201533, 201585, 194619, 59005, 201638, 201650, 201678, 201597, 201375,
    201475, 206858, 206879, 170576, 206905, 206919, 206934, 206950, 80419, 206967, 206984, 207000, 75800, 75811, 206991, 201732,
    166794, 201754, 62756, 62766, 201776, 201611, 201800, 201825, 201850, 201872, 207026, 83556, 202551, 178161, 207045, 207055,
    58304, 106993, 117454, 117462, 150716, 207068, 207076, 207094, 207112, 207131, 207141, 207155, 207201, 207212, 207223, 207265,
    17419, 17428, 207275, 207292, 207301, 207311, 207329, 207350, 17437, 207369, 125532, 153821, 153828, 207270, 24317, 174369,
    207388, 207407, 148091, 207427, 207449, 207470, 206373, 35316, 24660, 207490, 207504, 24705, 24432, 207519, 24470, 24479,
    66535, 93045, 207560, 139688, 165303, 170855, 207571, 207578, 141080, 189806, 207565, 207587, 207599, 207613, 146978, 207628,
    207648, 207637, 146988, 207658, 207687, 40442, 207708, 207724, 167408, 205684, 207752, 207764, 148101, 207777, 207800, 207832,
    207814, 207859, 207846, 207885, 207907, 99088, 207941, 207953, 207967, 207980, 207996, 157429, 208013, 208029, 208017, 133443,
    208077, 208087, 208098, 208115, 208128, 173024, 201889, 208148, 205943, 208170, 204795, 184791, 74741, 151444, 208205, 76968,
    77000, 111071, 164159, 176812, 208222, 137524, 208232, 208249, 208274, 208309, 137533, 204398, 74750, 34123, 208327, 208338,
    208349, 208362, 208371, 208402, 208415, 208430, 197543, 161041, 190152, 206336, 208448, 118649, 208460, 208476, 208485, 197553,
    197565, 208496, 208506, 109010, 208516, 208547, 208567, 208583, 208593, 208606, 208527, 109019, 208538, 208558, 206064, 67542,
    208625, 208639, 199026, 208670, 78185, 202623, 74553, 122331, 208681, 208687, 208693, 208702, 208712, 208724, 208717, 162234,
    103223, 45844, 208750, 45855, 208756, 208769, 208782, 125496, 162245, 208763, 208776, 208789, 208795, 208806, 208852, 46203,
    121779, 207072, 208873, 208884, 208896, 208917, 208923, 208903, 208909, 94541, 208936, 202122, 208956, 208968, 63849, 208978,
    202180, 208994, 209006, 209017, 209030, 209056, 141098, 209065, 90382, 209040, 8892, 209076, 41058, 41069, 209100, 209111,
    195602, 200567, 51540, 209127, 53976, 205989, 206014, 209142, 45054, 209153, 209164, 66540, 66547, 187890, 199068, 59083,
    187895, 187931, 199872, 199895, 83328, 199073, 209191, 209173, 209203, 74638, 195726, 177809, 193549, 209238, 209252, 209267,
    209273, 195730, 76802, 209284, 209297, 209325, 209303, 209308, 199997, 209365, 209381, 209389, 209407, 209369, 209374, 209423,
    198751, 209461, 209472, 209477, 36735, 173037, 87182, 52874, 175565, 203073, 80335, 209483, 209495, 202365, 209508, 209536,
    209565, 155541, 192184, 209595, 142662, 161512, 209488, 209602, 209609, 174316, 209619, 20267, 209156, 209630, 209643, 209657,
    209666, 209676, 209688, 209702, 209724, 209738, 187411, 209753, 70093, 176539, 209766, 209787, 196045, 175787, 147677, 209812,
    209831, 152111, 209858, 209840, 209879, 209899, 209920, 209953, 209992, 210014, 210024, 210061, 209847, 210089, 210109, 210119,
    210131, 210140, 210148, 210183, 210209, 111280, 210220, 210248, 43427, 209909, 210269, 210297, 210281, 210320, 210332, 210309,
    19276, 209931, 209966, 210344, 210354, 210366, 210379, 210390, 210400, 210188, 210411, 210198, 210427, 210438, 210449, 209943,
    209980, 210461, 209853, 196051, 96445, 210489, 138140, 210502, 210519, 210530, 210538, 55751, 20532, 154614, 210555, 210577,
    210563, 52462, 210598, 210605, 210612, 149270, 210625, 210638, 167540, 167862, 210509, 210667, 210688, 167493, 210631, 210644,
    167872, 210677, 210702, 210495, 210717, 67030, 196334, 196339, 153520, 153529, 177640, 184453, 210730, 210763, 101961, 125407,
    80990, 32362, 138202, 210778, 182793, 210785, 210794, 38393, 76618, 50728, 210808, 210818, 210830, 210837, 124438, 210844,
    210853, 210861, 130963, 210723, 52469, 210878, 210890, 210737, 210901, 210915, 210924, 38209, 210933, 205875, 205881, 197854,
    209245, 210919, 210951, 210959, 210970, 210991, 162613, 118439, 210885, 211007, 211017, 118327, 211036, 211063, 211026, 35635,
    118336, 211077, 211107, 211117, 144175, 40899, 208282, 44543, 144186, 208289, 40908, 75271, 211139, 209397, 150420, 211152,
    102256, 150430, 165874, 211163, 211173, 211187, 175095, 181788, 211202, 170340, 170350, 211231, 211256, 211270, 211261, 211275,
    211283, 66919, 160860, 211302, 188853, 188869, 211314, 41570, 211329, 41577, 211369, 211388, 199413, 63271, 211419, 211428,
    211439, 51680, 186622, 211339, 211348, 211360, 204402, 209314, 131561, 70735, 98376, 114507, 209319, 205649, 211449, 181151,
    211469, 211483, 211497, 210155, 61201, 211525, 211536, 211548, 211560, 211579, 209776, 211601, 209799, 211627, 210002, 20603,
    211671, 209889, 211614, 84296, 211695, 211711, 211740, 211490, 211755, 174721, 49435, 211771, 211790, 211810, 211184, 205333,
    211834, 124681, 204312, 93293, 211851, 211860, 82121, 211876, 211885, 211895, 211909, 211914, 121675, 98986, 211924, 176684,
    211938, 68393, 73910, 87861, 211953, 193303, 211967, 211975, 210214, 211981, 211988, 212008, 212028, 212061, 211997, 212017,
    212037, 212048, 212070, 212083, 212105, 212145, 212114, 27794, 212154, 24729, 212124, 50298, 212172, 212192, 76865, 125928,
    212212, 136890, 109433, 212232, 212251, 125936, 160389, 136899, 212223, 202631, 210163, 48407, 212277, 212290, 212309, 212341,
    212360, 212283, 212299, 212319, 212350, 210170, 48313, 76117, 212403, 124950, 124901, 124912, 210177, 65381, 212408, 124959,
    124919, 124977, 40, 52, 250, 90879, 91060, 128803, 212414, 212431, 212452, 212459, 40299, 212489, 125462, 211531,
    212499, 212515, 212506, 212528, 212540, 212558, 204359, 204371, 212581, 212597, 212616, 212630, 136022, 212533, 212545, 212643,
    212652, 212674, 212659, 212681, 211901, 174984, 212703, 212718, 92343, 199906, 212739, 53952, 199879, 212647, 212666, 212688,
    11507, 78302, 182800, 212770, 211717, 211721, 99545, 212787, 212796, 212805, 212822, 212835, 212842, 212850, 212858, 212866,
    212880, 125094, 212888, 125112, 125122, 212908, 212946, 32002, 212958, 208173, 212970, 212987, 213004, 205101, 167000, 119446,
    119461, 167020, 208152, 183821, 213021, 213042, 213055, 213070, 198168, 198191, 143244, 32292, 213031, 213088, 32237, 212979,
    213108, 213141, 213170, 39990, 213078, 212829, 213217, 213226, 213237, 213247, 98043, 213275, 213292, 213303, 213316, 213329,
    213351, 213372, 213393, 213416, 213436, 213463, 213486, 213510, 213540, 213500, 213565, 18767, 18783, 18798, 141655, 25345,
    163713, 213383, 213612, 213623, 213678, 213721, 175331, 213740, 213751, 175342, 213761, 213793, 213809, 162557, 213832, 213844,
    213856, 213870, 147940, 213886, 213798, 213911, 213935, 213949, 213962, 213998, 214022, 214040, 214010, 214058, 113508, 113519,
    214079, 214092, 112207, 214104, 214113, 214124, 214141, 3386, 12230, 1754, 92661, 186784, 187916, 187950, 56714, 214149,
    214168, 214184, 214157, 177531, 214204, 96794, 96852, 187954, 122740, 196294, 214221, 214231, 214255, 214263, 214275, 214269,
    34210, 187548, 214293, 214296, 30342, 214302, 214319, 153018, 159765, 214308, 214329, 214339, 111305, 214391, 214419, 214439,
    214463, 214486, 214500, 214530, 214553, 214563, 214575, 214586, 214596, 214606, 183793, 93654, 4556, 121989, 214618, 173616,
    199739, 76811, 214645, 195757, 214675, 10919, 214684, 214702, 10928, 214693, 214711, 214720, 93228, 214732, 214750, 214756,
    166679, 178556, 214761, 77802, 163610, 214790, 77812, 214798, 214814, 214837, 214825, 146442, 146543, 214863, 214883, 214922,
    214967, 214994, 215002, 215012, 215020, 140513, 215027, 215034, 158933, 16867, 214226, 214236, 1105, 215044, 215056, 215062,
    215073, 215098, 50947, 215119, 215165, 215132, 215186, 215047, 214872, 215231, 215252, 112813, 215301, 215260, 215273, 215321,
    215346, 163740, 215333, 215370, 215393, 215404, 113741, 215416, 215443, 215452, 137315, 169991, 90336, 198686, 166718, 1225,
    1357, 1480, 158420, 214241, 215461, 111190, 174575, 215475, 215497, 215502, 215508, 39725, 215536, 215558, 93660, 214626,
    214635, 160492, 4133, 38655, 153405, 215578, 215588, 215599, 142488, 195363, 195373, 215516, 173954, 120694, 156986, 29456,
    215631, 175924, 215653, 215694, 215715, 172170, 215735, 215758, 215781, 215810, 215829, 215855, 215894, 215932, 215956, 115742,
    216007, 215942, 215966, 73915, 164046, 164058, 215868, 78055, 216037, 216052, 62845, 216081, 175482, 216102, 216113, 216129,
    216144, 135812, 216157, 216176, 216203, 216189, 216213, 216242, 216259, 216269, 214735, 216290, 216302, 216316, 216334, 216352,
    180383, 93540, 180396, 74642, 216369, 216424, 216438, 13949, 161222, 216454, 216467, 216510, 87127, 216532, 216555, 34284,
    132750, 216577, 216594, 175972, 175935, 216611, 216629, 216669, 216702, 216739, 216750, 103233, 216796, 153579, 216061, 216071,
    216809, 216842, 137785, 216825, 216873, 216885, 216897, 216933, 216957, 174261, 216985, 217022, 217034, 146056, 150773, 217048,
    2529, 217064, 217081, 161360, 217097, 217110, 217123, 217145, 217160, 217176, 215640, 34047, 95411, 172181, 217195, 217248,
    217282, 217308, 217320, 216858, 217331, 70745, 217343, 217357, 217375, 162111, 217395, 217383, 33450, 217413, 160498, 217465,
    217493, 217508, 217524, 163223, 111375, 217561, 217584, 217604, 33457, 56938, 114349, 217627, 217646, 43056, 125627, 217662,
    217634, 217653, 49824, 148720, 217684, 217722, 217746, 173890, 173899, 217762, 217772, 140088, 217796, 217831, 217813, 16750,
    217878, 217892, 111336, 217904, 217929, 217978, 218006, 218022, 218039, 138782, 158105, 218051, 218071, 218085, 218101, 197588,
    138793, 218014, 85016, 218120, 218149, 218186, 218232, 218134, 171667, 218252, 218285, 218298, 9527, 212096, 218310, 218340,
    218365, 218378, 95424, 217207, 218399, 206779, 218417, 217365, 70755, 84635, 153596, 218434, 218455, 153787, 37349, 44960,
    218470, 218478, 218489, 218499, 218519, 218550, 218576, 151471, 218627, 218654, 218678, 218695, 218708, 135126, 218727, 218753,
    218776, 218738, 206159, 206172, 5806, 218795, 218819, 218843, 218867, 218893, 37615, 72318, 218917, 218937, 218964, 219003,
    219060, 219084, 219109, 219133, 219164, 44628, 219099, 219192, 219214, 219240, 219262, 219285, 219312, 219298, 219333, 12184,
    215077, 219372, 219397, 219431, 113162, 113800, 219464, 113807, 115007, 219479, 219495, 81069, 219484, 194981, 83108, 219515,
    219542, 219528, 206752, 124506, 65485, 44082, 161818, 87671, 219564, 219574, 36213, 179454, 219589, 219644, 219683, 219704,
    90045, 89974, 90056, 218473, 95952, 219731, 219693, 95960, 219750, 219766, 5829, 6147, 219775, 73637, 219740, 163462,
    137866, 9071, 219786, 219810, 219819, 219828, 179475, 219600, 203651, 219840, 219870, 219891, 219904, 219923, 147622, 102709,
    162007, 219937, 219964, 219950, 219984, 220013, 220045, 220071, 220084, 77234, 219552, 220097, 220121, 220143, 220165, 220188,
    220218, 220252, 215907, 217695, 220285, 220297, 215287, 220313, 220326, 220342, 220369, 220387, 145832, 145545, 220419, 93477,
    173861, 220443, 220471, 220457, 148277, 220511, 137262, 220576, 220531, 220605, 220645, 220688, 220717, 118725, 220729, 215746,
    215795, 175993, 220768, 220792, 175946, 220815, 220828, 9509, 220842, 220861, 220890, 220875, 220851, 123993, 124007, 56971,
    162188, 125987, 162209, 74280, 61688, 220914, 220926, 220938, 162255, 220961, 220974, 220988, 221016, 111037, 111049, 221026,
    29226, 221035, 221052, 221110, 221159, 221170, 221061, 221185, 221207, 221222, 221232, 221280, 221249, 221325, 221392, 221415,
    221486, 221510, 221525, 221584, 221196, 137885, 221605, 221617, 31768, 43714, 177076, 186954, 221627, 221644, 221427, 155166,
    221663, 221536, 221693, 221550, 221568, 185065, 57574, 21942, 218481, 210869, 134125, 221720, 221731, 94271, 29347, 221778,
    3090, 3098, 43246, 43278, 140101, 221813, 221829, 219653, 25658, 25665, 124609, 107315, 221846, 221884, 221910, 221939,
    221973, 167284, 221990, 16194, 28607, 146999, 222018, 220485, 222086, 222104, 64846, 214509, 214401, 222127, 218492, 222163,
    222210, 222229, 222247, 222303, 222345, 222359, 214539, 222317, 222262, 222331, 222375, 222419, 222440, 173634, 222427, 222459,
    222488, 162828, 162846, 222512, 222530, 222536, 222034, 222543, 222561, 222580, 222601, 114127, 222591, 222570, 222612, 222621,
    222650, 222668, 222677, 81626, 222688, 222703, 163055, 203805, 143317, 143333, 137695, 17454, 154077, 154108, 222718, 221441,
    222737, 222782, 222813, 222849, 222873, 189285, 222794, 222825, 222889, 222904, 222920, 222938, 222861, 222949, 222973, 223003,
    44634, 176416, 223039, 223054, 223079, 223118, 223137, 223157, 223167, 223185, 223199, 218719, 223015, 223227, 223252, 215920,
    223276, 223290, 223305, 223318, 223330, 223355, 223379, 9275, 223423, 223393, 223482, 223495, 223508, 95091, 223527, 223541,
    20795, 220205, 217593, 223553, 217916, 223581, 223595, 223608, 223649, 223664, 223677, 45281, 96646, 184673, 161681, 223696,
    217845, 223724, 223740, 223751, 223761, 223791, 215107, 135161, 223812, 61669, 137197, 206534, 223831, 223859, 192376, 223874,
    141363, 120914, 223900, 223919, 69242, 223934, 69255, 133238, 211292, 164283, 2755, 180934, 180959, 219609, 213427, 223955,
    223973, 223964, 224072, 224101, 224134, 224161, 224086, 224194, 224214, 224224, 121842, 156436, 224233, 224250, 164836, 163753,
    224278, 224302, 224290, 57232, 224326, 3962, 224352, 224380, 162119, 25196, 169082, 172087, 102422, 102443, 102457, 36855,
    36909, 224408, 224414, 146454, 163401, 197145, 217475, 224431, 104946, 224444, 180347, 224423, 224469, 224479, 199400, 224487,
    201163, 137705, 224510, 224519, 224530, 224548, 224540, 145678, 224569, 146734, 115015, 224595, 207437, 207458, 174380, 206381,
    206414, 206426, 224615, 224648, 106809, 224673, 224686, 77668, 224697, 224717, 54191, 224743, 224766, 164434, 219343, 224812,
    219354, 224833, 224846, 224856, 221923, 224866, 224880, 136030, 203959, 224904, 219490, 189868, 61364, 224925, 224934, 224942,
    224955, 110973, 224968, 224985, 41586, 164635, 68297, 113169, 225002, 225033, 225057, 225081, 224992, 225096, 66924, 36923,
    225113, 225139, 199642, 11740, 199655, 199597, 120923, 121038, 120935, 23130, 225160, 225179, 225170, 225201, 225230, 225254,
    169421, 169252, 24098, 24121, 224975, 37683, 225283, 225329, 225353, 225383, 225418, 225435, 203887, 225447, 225481, 225492,
    163430, 163470, 126251, 225513, 208107, 225531, 225547, 106817, 225562, 81562, 221860, 225583, 225604, 225626, 225642, 225572,
    164289, 225656, 225669, 225681, 200725, 225663, 225700, 225039, 225726, 225742, 143779, 225770, 225791, 174327, 224457, 61785,
    225817, 61850, 68269, 61861, 68280, 101249, 225832, 225106, 95434, 41706, 218408, 225853, 222117, 225880, 48569, 225925,
    225954, 225976, 169016, 70714, 160284, 190062, 205560, 225990, 226003, 167428, 226028, 167435, 226035, 173306, 215980, 216948,
    101259, 225995, 226043, 226051, 215989, 179483, 219622, 179493, 224822, 226065, 226072, 226080, 39622, 226097, 226115, 224314,
    226133, 226152, 217536, 226168, 226200, 226216, 226232, 182401, 226266, 226301, 135137, 135192, 226327, 226347, 226336, 142191,
    115588, 226365, 226392, 221338, 200250, 226418, 226450, 226377, 226481, 226511, 226526, 226428, 226542, 164541, 164555, 226570,
    141140, 224948, 216481, 216521, 226587, 216495, 115024, 164562, 220742, 226610, 114700, 226632, 226654, 226667, 226681, 91688,
    226691, 226704, 205439, 226733, 31634, 9134, 226764, 98942, 226778, 226802, 226788, 4182, 136069, 139412, 163492, 226828,
    226842, 226854, 57354, 125132, 226878, 169479, 169531, 169498, 226926, 226958, 117483, 226981, 212895, 226938, 227009, 227034,
    227059, 111396, 227043, 227073, 162267, 78325, 225063, 225733, 227088, 227115, 163641, 227142, 187232, 227167, 227216, 227228,
    226971, 227179, 227152, 227240, 227192, 227255, 227278, 226890, 227309, 226903, 163294, 163862, 162506, 62854, 227329, 227343,
    62866, 10576, 111621, 211680, 225072, 225087, 227097, 227357, 227377, 171955, 227400, 227434, 227451, 227417, 211687, 128984,
    164393, 142494, 227470, 171131, 227494, 227522, 227540, 227586, 227509, 227609, 77676, 226996, 227627, 225340, 227641, 180105,
    227655, 227671, 134440, 134453, 86877, 24674, 227683, 227696, 163651, 227710, 227730, 227205, 164338, 69689, 174456, 132140,
    227747, 215175, 93876, 17464, 227782, 154119, 154144, 205361, 227793, 227806, 227821, 227837, 227852, 227864, 227366, 227876,
    227890, 9286, 227904, 227917, 162910, 164774, 164796, 41756, 132167, 159037, 139447, 92804, 227931, 227941, 164296, 50519,
    227953, 151298, 227988, 228014, 228040, 228002, 228027, 228061, 227289, 228076, 83681, 228119, 228150, 167085, 136702, 14217,
    19575, 136730, 164883, 228191, 162139, 220998, 228050, 228226, 145386, 228248, 228277, 228288, 228300, 228321, 168636, 228376,
    228390, 228405, 93509, 162159, 221007, 225150, 215487, 36705, 228416, 228432, 228461, 24253, 228484, 140301, 228512, 228498,
    99063, 145959, 228536, 25837, 163811, 222474, 228572, 3974, 217941, 228609, 226577, 228620, 228641, 228661, 220433, 228688,
    228674, 228708, 200266, 228723, 228753, 228790, 159047, 79086, 228806, 219472, 227389, 228821, 228834, 228827, 228844, 228852,
    102779, 228860, 228884, 228892, 228900, 214144, 228965, 228970, 228976, 228994, 229005, 229015, 229027, 229039, 229045, 229061,
    229010, 229021, 98386, 98405, 229077, 229083, 158914, 208214, 229090, 93402, 229114, 228997, 229165, 180528, 68207, 176190,
    229187, 71341, 154172, 229198, 154179, 229205, 229214, 203782, 92436, 124850, 203761, 229241, 52527, 229259, 229312, 229342,
    229273, 229370, 229390, 220593, 229414, 229440, 229451, 229467, 77322, 229495, 61699, 229524, 229554, 229587, 229570, 229611,
    229626, 229639, 229656, 229674, 195788, 217260, 229702, 229714, 229380, 229402, 58916, 122564, 169668, 26411, 229726, 229748,
    229737, 228069, 229767, 229786, 229793, 229800, 229826, 229836, 229845, 37878, 87895, 229859, 229874, 229889, 229911, 229916,
    229922, 229930, 19196, 58708, 229940, 229962, 229973, 229985, 229995, 210259, 229809, 230005, 230018, 229852, 230033, 230043,
    230055, 230078, 230066, 230090, 224491, 229817, 230100, 80505, 230122, 230141, 230130, 230151, 230160, 230170, 230181, 230199,
    205919, 114309, 170501, 230224, 114894, 48078, 230114, 230248, 230255, 29047, 230261, 230283, 230290, 214725, 193707, 230297,
    230318, 186201, 186276, 66753, 230340, 163881, 230351, 1623, 944, 230359, 142688, 230372, 230381, 153549, 230401, 97125,
    212814, 230419, 70144, 207732, 132195, 125594, 230457, 230471, 230491, 230501, 230513, 117253, 230523, 11513, 201180, 230535,
    230550, 230540, 230564, 230577, 230570, 130311, 230594, 230604, 230615, 230636, 166426, 230661, 230644, 230652, 166433, 230668,
    230682, 172977, 172988, 230697, 230704, 102664, 190670, 85027, 190686, 102669, 190693, 230767, 230772, 230467, 149656, 230780,
    89891, 78091, 122354, 125890, 216709, 230801, 198709, 230816, 230845, 190264, 230864, 230878, 230892, 122358, 230871, 230914,
    230923, 230930, 230949, 230968, 230979, 230972, 230988, 230995, 7145, 214283, 231003, 72506, 131413, 97308, 131360, 105884,
    231021, 231027, 231036, 48318, 181172, 121467, 231079, 137653, 137664, 231099, 231119, 182990, 138922, 19643, 230711, 10752,
    218764, 225825, 231134, 231154, 231145, 231165, 113021, 113028, 191722, 191751, 231202, 231214, 231221, 166380, 231227, 180145,
    35961, 231241, 26880, 35969, 98695, 208441, 65385, 231262, 184249, 231272, 231302, 231309, 231318, 98801, 186124, 200675,
    231329, 182935, 231346, 72016, 205929, 191841, 52389, 231382, 52397, 231390, 231399, 231409, 90231, 231417, 231434, 231448,
    230719, 200877, 200881, 231106, 231112, 96542, 231460, 231478, 231467, 231490, 187217, 52215, 231511, 122041, 122050, 231535,
    231208, 231553, 231581, 231590, 94942, 94889, 94948, 94981, 149690, 149697, 231597, 231625, 208121, 43163, 157770, 231636,
    231650, 40322, 231678, 231683, 231280, 212373, 212384, 231703, 231287, 231717, 49088, 102805, 152179, 231497, 51347, 231752,
    199088, 231787, 231799, 231805, 36590, 113591, 121167, 231812, 36571, 36595, 231826, 231846, 231858, 231878, 231893, 231884,
    231295, 231906, 231913, 231724, 230983, 231921, 77080, 77088, 231630, 231935, 113043, 231964, 230583, 231974, 208811, 231987,
    231993, 68926, 141522, 232000, 232015, 36673, 232034, 231730, 117151, 231044, 159227, 232047, 231640, 230390, 159234, 172694,
    232060, 232071, 232065, 231441, 232084, 232091, 232100, 232106, 232078, 168399, 232121, 232142, 232152, 145935, 232174, 205380,
    232201, 232238, 232242, 230625, 232249, 232272, 232281, 232314, 232337, 154196, 153413, 232350, 232364, 175706, 190168, 232396,
    177647, 231969, 232432, 232452, 232459, 232466, 147282, 159742, 232482, 232504, 77094, 157778, 232519, 232550, 36601, 113696,
    36610, 232565, 232608, 231643, 232649, 133385, 232670, 232677, 9478, 232687, 9485, 232694, 232700, 232721, 232728, 232039,
    232735, 232183, 232750, 232191, 232758, 232127, 232741, 232768, 232774, 232782, 166001, 232792, 232134, 232805, 232822, 232832,
    134978, 232843, 134992, 232857, 30064, 232813, 232883, 76122, 232909, 232925, 10759, 232983, 232917, 131501, 79162, 79222,
    79261, 233013, 155232, 92526, 80147, 80154, 91518, 80167, 68164, 106857, 79510, 80542, 80552, 68169, 106862, 233025,
    233039, 49492, 80997, 81292, 68311, 81298, 233055, 81803, 82680, 82253, 82273, 59389, 233076, 233094, 82865, 82881,
    233113, 233126, 233152, 233170, 233180, 233190, 233196, 233203, 190573, 233086, 233213, 233231, 233238, 85151, 233246, 73887,
    85295, 233264, 86202, 233288, 233305, 233315, 233326, 233341, 87632, 87736, 87878, 88131, 88159, 88179, 233364, 233376,
    233390, 233400, 233409, 233428, 233434, 233440, 233445, 233454, 233472, 233491, 233504, 233497, 233510, 233517, 233528, 233538,
    233554, 233579, 233541, 191681, 233019, 233591, 233606, 85943, 86377, 9080, 128726, 171605, 150904, 128376, 233623, 91483,
    91490, 91497, 233653, 233657, 233664, 49497, 233671, 90694, 233677, 233699, 59395, 34986, 233710, 231452, 233719, 150206,
    233728, 233740, 91615, 127418, 233272, 233280, 127433, 233754, 233766, 233775, 233784, 233792, 89161, 233801, 233834, 233860,
    66014, 233878, 233921, 233885, 233928, 205346, 233938, 189497, 233955, 233963, 81305, 81312, 233061, 233068, 85341, 233969,
    233983, 233991, 88135, 97255, 233999, 78165, 96007, 233557, 234023, 233561, 234031, 234037, 130452, 96566, 96633, 234049,
    234061, 234069, 234076, 2311, 2341, 2350, 234053, 141618, 234082, 213335, 129077, 234093, 234110, 234122, 234129, 234101,
    197341, 234136, 234150, 234172, 233941, 233546, 233570, 233582, 81776, 233946, 234191, 234201, 234211, 234221, 234240, 234252,
    16931, 174472, 234264, 233139, 192200, 234289, 234303, 234318, 163618, 206708, 234344, 56244, 185525, 16940, 89194, 234277,
    163128, 91410, 49776, 234381, 234392, 234401, 78691, 121719, 234418, 91847, 95976, 234432, 234441, 234448, 58674, 205312,
    234410, 129746, 122728, 234466, 234477, 187830, 234492, 234513, 234529, 174476, 67657, 234008, 234551, 234559, 234569, 234016,
    234581, 234588, 234597, 97474, 78251, 120430, 234619, 234636, 233812, 233823, 234647, 187781, 234660, 234654, 234673, 234667,
    32147, 234699, 234714, 101948, 101988, 234231, 233222, 103778, 234723, 234237, 152824, 234730, 234752, 45892, 234761, 122284,
    234818, 234824, 234831, 192489, 234845, 234857, 234865, 72412, 63993, 72427, 234881, 234923, 234892, 234942, 234953, 9085,
    128731, 211145, 97583, 97612, 180903, 105278, 234966, 234971, 234980, 234989, 235010, 195447, 235022, 235030, 235037, 235048,
    76936, 66069, 67174, 138673, 151194, 103384, 53288, 234741, 202639, 104403, 77713, 195427, 198319, 235058, 195455, 110463,
    46660, 79095, 195463, 235076, 48323, 106094, 31603, 136081, 235098, 235120, 235159, 106113, 224777, 235106, 235128, 235172,
    224794, 235193, 235202, 84005, 117662, 50851, 235209, 80951, 107761, 110997, 112730, 235230, 235236, 235243, 117638, 117645,
    235262, 235268, 210020, 117892, 118188, 120061, 235275, 235285, 119714, 235296, 235301, 235250, 235307, 233416, 233422, 235321,
    192504, 235335, 235346, 235355, 235364, 122172, 122179, 65391, 65397, 123624, 123864, 124831, 235338, 233210, 85301, 83931,
    235257, 235373, 127250, 235388, 235315, 126996, 235381, 117924, 112170, 112178, 127724, 235411, 128448, 235328, 234249, 234261,
    235433, 83941, 200102, 150952, 233737, 128877, 234065, 234116, 129359, 129406, 124931, 234608, 234087, 235443, 235456, 235469,
    235482, 235497, 235514, 235524, 235532, 235548, 235577, 235596, 235486, 235612, 235629, 235655, 153932, 34365, 235672, 235711,
    235620, 235637, 235663, 153941, 235723, 235736, 235756, 235771, 235784, 235802, 235818, 163662, 235835, 211505, 235847, 235858,
    187711, 235872, 235891, 235881, 235900, 235923, 235941, 235961, 235979, 235998, 236029, 150468, 235840, 187691, 153052, 123060,
    183312, 3888, 135215, 236054, 236065, 127868, 235989, 236010, 236042, 236098, 236106, 236114, 236128, 236145, 207168, 61930,
    80275, 235867, 236163, 62996, 126556, 235506, 126560, 236204, 236215, 236209, 236220, 236232, 236253, 236238, 86287, 236226,
    130922, 236269, 236293, 236304, 236313, 236321, 154655, 188119, 236341, 236349, 132210, 236357, 236367, 236373, 235476, 132242,
    143216, 132250, 236386, 236393, 236400, 236407, 137545, 236424, 236434, 126, 236443, 77258, 180747, 159873, 83301, 83309,
    194311, 15444, 236456, 124481, 176569, 193536, 60623, 236485, 60656, 138310, 199127, 236522, 236548, 235586, 236562, 236570,
    236578, 971, 236598, 236610, 124715, 236622, 236641, 70267, 236657, 179908, 236671, 236684, 22727, 236696, 236706, 236677,
    236717, 236737, 236745, 128741, 128751, 236761, 236782, 236810, 236837, 236793, 236772, 231124, 236859, 236870, 236801, 236821,
    236882, 123014, 123092, 236900, 236919, 236927, 153066, 181293, 153074, 236935, 83409, 236951, 190412, 190434, 143016, 236964,
    236970, 236977, 236984, 236991, 237013, 237024, 237035, 235446, 236943, 236753, 237045, 237049, 237057, 237066, 237078, 237089,
    237098, 237116, 237131, 237107, 237148, 237161, 237178, 237216, 215611, 237169, 237183, 201236, 159103, 237230, 16762, 237252,
    237273, 237287, 237296, 237306, 78802, 58657, 171892, 237329, 58446, 237366, 237374, 58461, 237422, 237447, 89851, 237476,
    237492, 56061, 22561, 237511, 237531, 237539, 237547, 99909, 182582, 237582, 237593, 188980, 237603, 237615, 237608, 218974,
    237429, 237627, 155214, 237638, 237656, 155146, 54962, 185715, 237675, 237686, 237680, 237698, 237723, 150648, 202828, 237740,
    237754, 182587, 237767, 237788, 237761, 237802, 208987, 237816, 237834, 237873, 237886, 187273, 199276, 187243, 187252, 187281,
    237898, 31071, 237910, 172063, 237940, 172072, 4564, 23248, 237969, 195635, 88465, 237984, 237997, 238020, 238009, 238065,
    238096, 238030, 125545, 125554, 238117, 51352, 202726, 238137, 238146, 238156, 50632, 51356, 238167, 238182, 238211, 238273,
    238317, 121340, 238337, 238361, 238377, 24025, 238395, 238410, 238386, 238128, 238431, 41441, 238445, 238455, 238462, 236412,
    238491, 238513, 76817, 53471, 238524, 238545, 238552, 238570, 238582, 238590, 238600, 222450, 237880, 45161, 173643, 231795,
    238620, 238648, 238664, 238684, 238700, 16063, 238716, 238735, 238771, 184008, 184013, 238789, 128199, 238803, 60972, 40392,
    219852, 170369, 238821, 238849, 69153, 111917, 166662, 235142, 31730, 38713, 94547, 238861, 238880, 238891, 118149, 238904,
    238930, 238916, 238942, 118159, 238956, 129590, 238975, 118171, 170519, 238796, 239013, 239030, 239020, 138068, 239043, 126366,
    239048, 239067, 239074, 238674, 239083, 71945, 132273, 152336, 239095, 63860, 239107, 239137, 63879, 239180, 239194, 239239,
    239099, 19405, 32708, 239262, 239277, 239267, 239316, 107607, 239282, 239327, 239342, 70896, 239357, 158271, 173092, 15116,
    239372, 65182, 144989, 239387, 239380, 239410, 239426, 239436, 42658, 238344, 239448, 173073, 94084, 239467, 239484, 239476,
    239496, 239507, 239517, 239532, 239546, 239565, 239592, 156543, 239605, 178908, 239624, 190079, 239638, 239652, 239662, 239688,
    148374, 155278, 160196, 115834, 171853, 239702, 239713, 171793, 222632, 239724, 222641, 239733, 222660, 239758, 18965, 239742,
    239788, 239793, 239799, 239820, 58056, 58067, 186869, 186881, 186852, 239832, 239847, 239856, 234538, 239873, 239889, 239905,
    238576, 239925, 239951, 239965, 239938, 239983, 150224, 239992, 240022, 240033, 240045, 240059, 240072, 240080, 240096, 240112,
    183598, 51187, 238873, 143478, 240119, 239055, 132669, 240135, 181301, 218984, 237893, 235150, 240157, 240181, 240195, 240209,
    97765, 240167, 240220, 239251, 240231, 240251, 205451, 240259, 231559, 240268, 240286, 211746, 240299, 56070, 240318, 240329,
    188129, 237558, 240342, 51984, 240373, 240385, 240397, 240424, 240438, 240411, 238624, 239539, 214260, 237621, 1167, 240454,
    32115, 240465, 240507, 240526, 176946, 45645, 81897, 81907, 58570, 240544, 240575, 240552, 240592, 240603, 160313, 133130,
    133169, 237563, 57849, 97821, 97831, 6571, 6580, 240615, 239145, 240641, 240655, 239152, 238355, 51234, 237061, 240673,
    93969, 172914, 240692, 240710, 177913, 240730, 177919, 240741, 240753, 240778, 138848, 240796, 240833, 169060, 218951, 197732,
    240843, 240853, 67555, 240869, 204509, 236380, 59470, 76007, 215358, 240889, 49439, 214806, 49463, 16494, 240901, 63293,
    52278, 240922, 15125, 240962, 240986, 63306, 241011, 15135, 240972, 230430, 1173, 1180, 30769, 241030, 18196, 109839,
    241047, 241057, 30777, 241038, 241070, 241081, 241092, 241109, 18219, 241125, 238176, 162650, 241144, 48686, 178364, 241187,
    241198, 241231, 241240, 35241, 241249, 50867, 59909, 82021, 69089, 136874, 241263, 241269, 100580, 171452, 241280, 102866,
    112454, 69208, 215198, 241297, 69266, 241317, 136370, 239631, 241346, 241351, 193556, 241361, 209260, 126425, 241373, 101675,
    241416, 180473, 241451, 241466, 241484, 241504, 241517, 241493, 50872, 50888, 241528, 79453, 124318, 136343, 241543, 24384,
    241458, 176380, 9545, 23687, 160170, 241557, 241572, 241590, 159493, 241603, 159501, 74418, 77437, 121553, 121563, 132221,
    241616, 241629, 121575, 121596, 101580, 241647, 241666, 241675, 241684, 241655, 160752, 241609, 113822, 113915, 241702, 241714,
    241724, 121174, 241732, 196011, 241707, 241740, 241749, 241757, 241774, 241779, 181771, 205161, 241764, 239827, 239851, 239556,
    239580, 241788, 73200, 241818, 241828, 241840, 241846, 40726, 241860, 241889, 235604, 241853, 241906, 241917, 241869, 241878,
    207084, 241928, 241952, 241972, 241999, 118054, 242018, 242056, 242086, 62901, 183180, 62912, 183190, 241940, 241962, 241982,
    242106, 67924, 240682, 240076, 242123, 242130, 242137, 242165, 242172, 242211, 206239, 219860, 41175, 242244, 147564, 242265,
    215366, 240897, 239158, 239166, 240648, 242306, 8963, 167051, 242180, 242223, 242186, 240457, 195392, 167059, 242318, 192764,
    240786, 242336, 242345, 108481, 242355, 242312, 53618, 167246, 242378, 104303, 242388, 242397, 242428, 242449, 242363, 1188,
    242465, 18415, 69034, 211423, 242482, 242473, 240801, 242498, 17700, 69039, 242520, 242532, 18442, 40780, 58888, 207121,
    242545, 207102, 58897, 37494, 7631, 7640, 242566, 242583, 242604, 14052, 236246, 149864, 77471, 86294, 242613, 242631,
    76826, 239695, 28618, 242651, 3061, 242672, 242742, 242810, 242825, 242817, 27212, 242832, 125282, 242838, 242848, 242864,
    242884, 242900, 242919, 242939, 242929, 242984, 243008, 243016, 127651, 163268, 227162, 227250, 79440, 184562, 243024, 243035,
    243043, 243054, 243075, 163354, 243095, 243106, 123206, 151323, 243116, 243123, 242907, 8184, 242622, 242641, 41590, 243131,
    65038, 211703, 243141, 142819, 243149, 168013, 41389, 208615, 243158, 159776, 161054, 243188, 243209, 69846, 174218, 63486,
    106208, 106233, 239459, 243220, 243231, 243243, 243264, 243273, 243252, 243284, 243298, 243310, 243319, 243331, 82616, 132410,
    34784, 243346, 72362, 243358, 241638, 243370, 243394, 243379, 236892, 243410, 243420, 19688, 243429, 243441, 19768, 19693,
    243434, 243465, 243481, 243446, 243472, 19779, 175152, 243501, 243539, 243546, 184827, 184859, 55402, 243565, 243589, 243609,
    149871, 175157, 175191, 243634, 243654, 243680, 103579, 243695, 243701, 37929, 53297, 243714, 243754, 243771, 243791, 9667,
    149318, 123141, 123148, 106464, 124281, 219759, 243842, 243855, 146357, 243873, 243888, 139900, 102819, 145124, 191883, 243924,
    243938, 37731, 243959, 94807, 190969, 190984, 40421, 19651, 243981, 244001, 244015, 36065, 142997, 42030, 146066, 144038,
    240244, 244035, 82448, 173589, 138813, 28624, 242657, 244044, 31213, 19168, 185872, 64932, 71250, 226812, 244055, 235459,
    244075, 244086, 120304, 130646, 244098, 225520, 242756, 122745, 179028, 103119, 67466, 179063, 179070, 244115, 244132, 48328,
    55757, 244123, 75639, 53242, 56690, 10905, 140851, 244151, 244161, 13630, 13665, 239599, 69171, 244169, 244200, 244177,
    138689, 244219, 244247, 68757, 76751, 50305, 212179, 212199, 244275, 244307, 244316, 244325, 175496, 244341, 244357, 244375,
    244392, 221046, 244407, 238442, 244431, 96933, 103124, 236830, 236850, 244452, 244478, 244457, 244483, 198464, 244467, 244493,
    159909, 50603, 15452, 17796, 106241, 126432, 241380, 101685, 101694, 53997, 132372, 244531, 244541, 244552, 93136, 185957,
    244567, 176086, 244584, 189660, 241386, 238189, 244619, 4574, 174872, 243708, 244629, 244643, 46764, 194762, 242892, 240736,
    202296, 244656, 244671, 244703, 244726, 244732, 53304, 233102, 243213, 244738, 244752, 174225, 174236, 244772, 243351, 242607,
    244791, 73055, 244828, 195000, 73065, 15459, 73076, 103131, 99307, 103138, 212494, 244856, 149884, 244863, 99353, 123768,
    131569, 244882, 244904, 135830, 135836, 177119, 212552, 243558, 115215, 243641, 243647, 244923, 244930, 244399, 244937, 244943,
    85948, 244950, 47036, 164529, 244973, 7650, 193146, 244958, 135973, 232402, 135925, 244988, 245023, 73162, 245045, 245059,
    236076, 181339, 111711, 237574, 244504, 203611, 73169, 245052, 245066, 236087, 181354, 77893, 194483, 111719, 205607, 244624,
    245089, 245114, 244434, 245126, 245143, 245098, 245173, 244443, 245135, 245119, 245107, 245195, 136807, 245202, 245223, 245213,
    245236, 244997, 159001, 165429, 172632, 136763, 165439, 238284, 1195, 1201, 1208, 1216, 1242, 1254, 245199, 1291,
    1298, 1306, 1315, 1323, 1330, 1338, 1347, 1375, 1406, 1414, 1423, 1433, 1442, 1450, 1459, 1469,
    1518, 1541, 1550, 1560, 1571, 1581, 1590, 1600, 1611, 173740, 245248, 239173, 240664, 88591, 245277, 245290,
    245303, 245315, 129013, 241475, 88376, 245325, 245330, 245336, 245348, 245360, 245366, 245374, 245385, 245406, 245415, 245422,
    245433, 245444, 245464, 245510, 245527, 245542, 245566, 193180, 245596, 245622, 245633, 199923, 245651, 245657, 245663, 245668,
    245685, 245693, 245679, 66999, 245714, 105503, 43086, 43579, 245736, 78484, 245570, 245755, 245580, 245585, 245765, 245776,
    245782, 245792, 245807, 245823, 245837, 120031, 245852, 245873, 39423, 245900, 245928, 245940, 245954, 245985, 246006, 246018,
    246029, 246043, 135278, 135315, 131458, 246036, 70835, 136507, 203894, 211944, 130927, 70765, 131577, 25207, 169094, 53688,
    244890, 246058, 246085, 95993, 246098, 246106, 20559, 87399, 102839, 183614, 246124, 94426, 94432, 113753, 246140, 246203,
    43376, 148729, 237383, 198664, 197412, 49781, 246217, 246236, 94362, 246249, 246280, 246258, 246294, 246269, 191461, 64424,
    73119, 197603, 194794, 163694, 197461, 25308, 141951, 141992, 45249, 246308, 83856, 212424, 212439, 246325, 47427, 246336,
    246358, 246367, 246377, 246387, 246132, 246396, 150515, 150536, 246412, 47435, 246344, 246432, 246447, 246455, 246490, 246507,
    20613, 246517, 219365, 246522, 213281, 246529, 246540, 20618, 246549, 246556, 246563, 246579, 246595, 87022, 246611, 246628,
    73381, 96458, 221639, 246653, 121855, 246662, 246690, 246706, 246726, 246756, 246774, 246785, 118284, 190085, 246512, 47441,
    246350, 216994, 243194, 246797, 243168, 246765, 76246, 84475, 245703, 19656, 246815, 218389, 110605, 178890, 246828, 246849,
    246868, 246876, 149150, 246886, 246904, 95363, 246938, 246955, 246961, 246969, 246977, 144552, 246987, 246996, 247006, 6779,
    123932, 168749, 47397, 246895, 247039, 247060, 247072, 168757, 135526, 247091, 247115, 247084, 168768, 247144, 247155, 45653,
    247166, 247176, 247203, 247214, 247229, 154412, 154430, 54061, 202593, 202607, 247253, 177721, 247264, 247283, 247308, 247313,
    247320, 4701, 4730, 247328, 245467, 247337, 247350, 245472, 247387, 247394, 247343, 247404, 247358, 193445, 245478, 192231,
    126827, 126849, 202929, 247418, 6232, 247432, 104447, 144344, 247447, 247482, 247523, 247533, 247544, 247561, 247581, 110921,
    247571, 247589, 247553, 247599, 247628, 247640, 143889, 231049, 231058, 224341, 190829, 190837, 32994, 33002, 93807, 247651,
    247683, 247709, 247011, 247731, 247752, 247782, 247795, 247809, 247822, 247864, 235910, 246635, 32807, 247816, 212164, 244897,
    247876, 247883, 187104, 154324, 247892, 247903, 247913, 30543, 247922, 210472, 247455, 247490, 247941, 247948, 87824, 87839,
    190846, 87846, 247956, 247978, 212131, 27721, 247018, 247738, 247763, 247026, 247746, 247463, 247498, 175423, 183806, 210480,
    247473, 247508, 247999, 93816, 247667, 247696, 93826, 247720, 248017, 202391, 248038, 202397, 211566, 242067, 140656, 248050,
    96997, 248069, 248087, 97004, 149543, 248104, 248115, 248125, 248144, 248060, 248135, 248078, 248151, 102961, 97016, 248096,
    53309, 173374, 248158, 160335, 248180, 202676, 246114, 248192, 248207, 248214, 62818, 248224, 248232, 246499, 248241, 248275,
    248303, 248333, 248311, 248347, 248359, 248322, 248373, 4784, 67980, 170204, 248385, 248399, 184685, 248414, 248406, 248430,
    67986, 248451, 248470, 248486, 248497, 248506, 248516, 19412, 248541, 68036, 187726, 248558, 248574, 34952, 96032, 248491,
    245556, 83527, 190532, 83537, 190542, 248591, 238968, 248609, 195679, 248634, 248641, 248648, 248666, 248616, 248694, 248711,
    199387, 248729, 248753, 248804, 248736, 248845, 248854, 248862, 248870, 248877, 248899, 248912, 248925, 248937, 248945, 248955,
    248963, 248971, 248981, 248991, 249015, 249030, 229954, 249048, 249065, 249082, 249099, 249111, 249091, 248442, 249056, 249121,
    249131, 249141, 249156, 248674, 248997, 249176, 249186, 177017, 249197, 249205, 177023, 65837, 248748, 249214, 249256, 145212,
    249272, 249288, 174729, 218160, 249307, 174740, 218171, 249318, 96379, 199578, 204179, 249339, 66360, 246093, 249350, 248461,
    247048, 125900, 249362, 249374, 249389, 102995, 249403, 187014, 249430, 249447, 249456, 249475, 249410, 249418, 14871, 76260,
    249489, 245513, 249501, 234837, 249515, 249524, 249536, 76282, 206259, 249465, 249566, 249574, 115175, 244024, 115185, 70648,
    111211, 39363, 239840, 249583, 249597, 180544, 180600, 180614, 235746, 249613, 249622, 249632, 249640, 249650, 249666, 249686,
    249706, 245722, 117518, 155100, 249725, 249714, 249696, 249797, 249817, 249830, 103655, 118775, 249854, 69492, 249869, 118690,
    165905, 161743, 249893, 249920, 249932, 99241, 249955, 197809, 249978, 249844, 161758, 249997, 250009, 248682, 161767, 181369,
    250021, 181374, 250026, 250032, 250039, 240291, 181844, 250059, 250074, 250066, 250086, 250093, 250099, 247871, 250106, 210800,
    246422, 250131, 250164, 250142, 250176, 250186, 250201, 250221, 250240, 80340, 153437, 250255, 196390, 250275, 250283, 250293,
    207379, 250309, 250320, 250301, 250331, 250356, 250367, 250382, 250387, 132332, 250393, 250398, 235766, 46637, 235797, 191612,
    250409, 250425, 250450, 250412, 250462, 137557, 149800, 249359, 250476, 250494, 246659, 204967, 237481, 250529, 250536, 170378,
    250545, 170389, 250567, 123377, 174938, 250579, 250612, 250650, 250663, 189297, 250692, 53031, 250718, 250816, 250657, 250840,
    250854, 250884, 22515, 80793, 86061, 250906, 250935, 215208, 233761, 78498, 250960, 250978, 251000, 250428, 245746, 251014,
    251035, 251057, 251046, 251068, 251089, 28978, 251111, 251125, 251134, 251143, 78337, 228090, 78348, 251153, 196611, 251173,
    251192, 251199, 251206, 251161, 226917, 251221, 13136, 251242, 251253, 251273, 251284, 251264, 251227, 149803, 251294, 97592,
    196621, 251183, 251309, 44203, 74320, 115385, 227322, 251327, 251357, 76022, 251385, 251408, 251430, 251437, 251445, 251454,
    251471, 251487, 251497, 251518, 251534, 10270, 162070, 180200, 41125, 112576, 116650, 237454, 91564, 139, 250417, 250437,
    250453, 251555, 245862, 251570, 251585, 251597, 251608, 245970, 245996, 251621, 251633, 246210, 251645, 251659, 128947, 251674,
    251687, 251699, 251711, 251722, 251747, 251761, 251777, 251667, 251794, 251804, 251815, 251823, 21691, 125855, 251786, 251833,
    154622, 154634, 208453, 251855, 251862, 188307, 251871, 251879, 251897, 251915, 209087, 18586, 147038, 242116, 242010, 30932,
    251941, 153997, 234708, 251962, 251980, 251990, 252000, 47328, 252021, 131808, 252035, 250479, 252045, 180298, 54155, 187323,
    212185, 252056, 252065, 252072, 252082, 252091, 252098, 88318, 250895, 250915, 250926, 192212, 97078, 39769, 136944, 237465,
    252104, 140709, 140725, 252126, 250986, 165836, 165843, 252143, 252163, 252188, 252221, 186976, 249883, 252199, 252247, 252258,
    252269, 252294, 252307, 248764, 68704, 252316, 252335, 252356, 166499, 252385, 252174, 252394, 208185, 252329, 193484, 71259,
    252404, 252434, 220753, 112594, 252462, 139316, 252477, 252490, 139278, 252512, 252500, 139322, 61033, 61038, 248477, 212393,
    17707, 177515, 231737, 252528, 252556, 252579, 252116, 252590, 252611, 252628, 163273, 163848, 252651, 163306, 249382, 252656,
    252685, 252693, 252702, 252711, 252735, 252723, 252747, 252664, 252759, 213687, 175353, 213772, 175364, 213782, 213696, 252773,
    252792, 252783, 252804, 252815, 252836, 252765, 193915, 252842, 252851, 252859, 121239, 98838, 252868, 252889, 209715, 252901,
    252911, 142712, 252922, 252937, 252952, 252967, 19435, 252992, 253007, 252998, 253016, 253026, 253038, 253052, 186089, 253070,
    253086, 253116, 253128, 253141, 253152, 20488, 253165, 253177, 68020, 253188, 253211, 253228, 253265, 253276, 253218, 253237,
    253250, 253289, 253301, 253318, 195024, 92665, 251098, 180320, 180329, 92670, 185580, 253340, 191936, 92686, 247611, 253362,
    253384, 253411, 21985, 198344, 253430, 17604, 250499, 253452, 136659, 253476, 253535, 253489, 185883, 253558, 253568, 243948,
    253580, 253295, 253307, 253329, 250944, 253630, 250951, 253637, 253646, 180727, 100955, 205499, 190114, 253676, 244143, 253698,
    253709, 252597, 164818, 251564, 246246, 253717, 253730, 253742, 253754, 209148, 253762, 252135, 253770, 250446, 251008, 253797,
    229884, 253817, 253823, 253745, 253831, 253853, 253858, 253871, 253885, 50212, 75788, 253905, 253915, 253924, 253938, 253966,
    250466, 72616, 253983, 254025, 253995, 254039, 254010, 254055, 254073, 254105, 254121, 254066, 141034, 254172, 254186, 254197,
    253801, 254210, 254222, 167032, 254236, 252642, 91202, 202343, 251214, 254253, 242251, 115241, 227268, 231657, 254279, 251318,
    48698, 155638, 227124, 254298, 254290, 254306, 91216, 254315, 141466, 254260, 254268, 254322, 202350, 6440, 52959, 251526,
    26421, 254330, 254336, 254347, 254364, 254382, 254355, 254372, 254403, 253812, 253878, 89916, 254416, 254425, 254440, 254446,
    254453, 47835, 251459, 192466, 239749, 253757, 254420, 254432, 209335, 254468, 108505, 108515, 8859, 55546, 110135, 204897,
    254483, 254507, 254495, 254528, 254461, 254545, 130536, 254568, 254611, 254554, 173386, 199318, 254629, 96732, 96767, 96744,
    254645, 254656, 254667, 245530, 252147, 253720, 89920, 89931, 254696, 254707, 254137, 204635, 143, 52223, 254677, 254686,
    254721, 254735, 254748, 254757, 209346, 254765, 254576, 230825, 165855, 254787, 254828, 254834, 254792, 254862, 254868, 254877,
    47337, 254882, 47347, 254899, 254917, 32096, 45378, 44428, 206097, 185450, 254930, 254960, 206212, 219011, 254977, 254983,
    250727, 20714, 254994, 255008, 215383, 255023, 90586, 255037, 255056, 255064, 255072, 255091, 255013, 127342, 255111, 255119,
    255133, 196412, 255146, 255162, 255170, 96570, 255182, 255195, 255218, 243662, 231944, 106678, 255155, 255231, 255246, 255101,
    255257, 34158, 211541, 223948, 255267, 143520, 117820, 255281, 249328, 255295, 255319, 255351, 255365, 228815, 84779, 84794,
    103979, 255375, 255387, 23258, 231743, 65844, 255407, 129459, 255433, 129472, 255446, 255419, 160522, 51019, 51028, 207415,
    255461, 38850, 255483, 255523, 47409, 33465, 46508, 217424, 255540, 255557, 255577, 255000, 203198, 247103, 255587, 255604,
    255533, 255472, 255623, 255641, 255596, 133823, 255657, 255675, 255693, 255706, 36186, 255648, 255631, 247295, 148765, 178872,
    135536, 255717, 3896, 255736, 255746, 255757, 255768, 255778, 255792, 255807, 255785, 178877, 112750, 255614, 14631, 205048,
    4712, 255819, 149117, 239200, 74791, 208815, 255854, 255866, 191525, 255886, 255901, 255923, 255932, 255911, 255893, 18974,
    18985, 98452, 255944, 255860, 255876, 194949, 202682, 20721, 255964, 255975, 255549, 38864, 227621, 49595, 111634, 255987,
    256021, 255993, 256003, 256027, 256010, 153614, 89942, 254718, 247792, 255143, 256034, 256043, 256056, 255816, 256071, 55615,
    185968, 230805, 256085, 256101, 256117, 256109, 190518, 256159, 256181, 256190, 182997, 256199, 138930, 256218, 256206, 256244,
    175430, 169972, 247517, 256267, 256279, 256296, 256340, 256357, 256348, 91376, 256394, 256414, 256433, 256457, 256446, 256496,
    91385, 11751, 256248, 256518, 115484, 256255, 115494, 256529, 115504, 212205, 251464, 255188, 256470, 256540, 256551, 256559,
    241288, 93690, 237747, 96229, 153210, 204644, 256575, 256586, 256596, 49789, 256603, 246463, 246472, 256609, 256631, 256664,
    246404, 251302, 65975, 256678, 256685, 256692, 131088, 153629, 131103, 256671, 256706, 205196, 199331, 256366, 256715, 178226,
    256726, 256743, 256764, 256786, 256371, 256811, 256735, 256828, 72022, 120868, 256710, 256847, 40546, 256091, 256861, 256881,
    256894, 256902, 256888, 105658, 17153, 256854, 256920, 241992, 256933, 256074, 245484, 231710, 114356, 182030, 182043, 218425,
    245488, 245494, 256948, 256960, 256954, 25418, 256973, 256992, 257004, 257017, 257035, 257067, 257045, 257055, 257077, 257089,
    256998, 103286, 257109, 103295, 257123, 257010, 257144, 256981, 257175, 257193, 257201, 257209, 257232, 14059, 257251, 106685,
    257266, 244594, 252209, 179160, 49406, 257293, 257318, 257325, 50977, 127263, 257335, 257346, 257259, 253896, 10600, 257357,
    257373, 10694, 151910, 257118, 178266, 216373, 257387, 257415, 19174, 25017, 206618, 257444, 257133, 210234, 257460, 257474,
    257485, 257497, 172796, 57044, 162660, 162668, 241155, 241171, 257510, 257529, 257537, 257154, 257220, 257547, 257569, 257558,
    257579, 257165, 249544, 257590, 257602, 257449, 28401, 257615, 257627, 203268, 257640, 257652, 257663, 257675, 257690, 257705,
    257724, 257714, 257733, 212240, 257743, 212259, 257754, 214429, 54718, 137109, 186824, 257775, 91775, 257789, 186832, 40869,
    199521, 107050, 257812, 251971, 257823, 249555, 79896, 79902, 151956, 257843, 257863, 150440, 257888, 257300, 257868, 257902,
    67705, 257878, 67726, 244912, 153092, 257910, 56883, 56894, 257916, 56911, 257930, 257939, 257948, 56919, 257957, 257921,
    159958, 238987, 34737, 257974, 50280, 130654, 145221, 255324, 146134, 193267, 257988, 257998, 223771, 49193, 89523, 258025,
    258032, 258040, 257965, 258051, 258067, 79336, 249021, 249007, 93669, 151962, 258085, 258126, 258134, 192678, 258143, 258169,
    192690, 258155, 258181, 258206, 258215, 60207, 132917, 222174, 211764, 204803, 258223, 258233, 258243, 258259, 190455, 160400,
    190499, 136911, 160423, 258251, 258058, 258280, 21321, 258299, 229897, 258309, 183773, 40555, 38496, 175796, 249942, 258323,
    258336, 218636, 258352, 258381, 258363, 258329, 258374, 258392, 258404, 258417, 243086, 38870, 256966, 91255, 258411, 129093,
    255046, 258446, 43170, 43094, 43179, 58687, 129103, 258466, 158356, 258474, 43694, 251079, 258484, 258494, 258528, 171549,
    140500, 12240, 131485, 258545, 258554, 258562, 177136, 256379, 256720, 256753, 256385, 256819, 256834, 250266, 256940, 202645,
    208675, 122337, 258589, 94381, 102617, 199136, 150312, 258610, 56664, 123561, 258629, 258639, 258652, 258665, 258688, 258707,
    258646, 258659, 258728, 258745, 39216, 160705, 200497, 258734, 190814, 258760, 258774, 258797, 258785, 147582, 243063, 258805,
    13513, 258343, 258855, 204989, 45718, 256170, 258870, 258880, 254730, 255804, 251656, 257820, 258890, 258899, 258909, 195043,
    195098, 153741, 196187, 248187, 255356, 258925, 65849, 258946, 258958, 258965, 258974, 258981, 102000, 255361, 88912, 196648,
    143746, 103914, 258988, 9620, 9576, 65855, 103985, 143754, 258952, 259002, 258935, 259013, 259025, 258917, 191851, 259041,
    259080, 131120, 259103, 99269, 120437, 259120, 259124, 184261, 259131, 177088, 246481, 256620, 256642, 256653, 258569, 177144,
    259151, 259162, 176926, 203506, 259179, 256868, 256874, 259224, 259256, 259278, 231425, 255497, 255510, 149410, 259299, 259327,
    259350, 247931, 259371, 259408, 259420, 259433, 51590, 120946, 259444, 245500, 66513, 259467, 254798, 245504, 259493, 259504,
    168659, 259516, 259532, 259552, 259587, 259564, 259599, 257306, 257311, 259034, 237395, 238218, 35479, 185925, 259636, 259654,
    259659, 259665, 245453, 242077, 210099, 250153, 103924, 103991, 255381, 259475, 259676, 259707, 259745, 259770, 259779, 249482,
    259790, 259806, 259822, 259797, 259813, 74988, 71582, 259836, 259853, 65714, 65728, 259843, 71620, 252827, 71589, 163143,
    259860, 259231, 150258, 259869, 153449, 259879, 251889, 259899, 259910, 226184, 74331, 249396, 259829, 259922, 259933, 259943,
    229904, 258316, 259951, 259960, 258677, 252895, 256796, 258698, 259972, 258718, 259982, 233256, 259994, 260003, 260013, 256801,
    188067, 256841, 260033, 93424, 260044, 255954, 260056, 260063, 154683, 260088, 212725, 259088, 260122, 246618, 246643, 127271,
    259484, 258767, 260139, 227024, 258575, 258582, 260157, 260164, 189573, 9628, 62668, 250491, 254745, 250364, 77742, 260172,
    260185, 260199, 260209, 260221, 260248, 251393, 251419, 251400, 260255, 260263, 229866, 260277, 254888, 254893, 260303, 260313,
    260336, 260368, 255288, 260403, 260410, 112, 120960, 260267, 260281, 260292, 144021, 155998, 260420, 260442, 233298, 126263,
    260461, 260472, 260502, 260479, 260509, 252982, 126274, 173198, 173205, 173214, 173225, 240126, 213577, 213590, 213601, 260518,
    260556, 260529, 260587, 260571, 260542, 200912, 260618, 260644, 260654, 31582, 260663, 260675, 260685, 229125, 118130, 260704,
    260711, 260721, 201185, 260733, 260737, 260758, 260775, 18384, 260797, 260813, 260825, 31586, 62137, 76595, 260830, 260836,
    260852, 260885, 260914, 260924, 260932, 260786, 260819, 260940, 134962, 260950, 260961, 177188, 260980, 260988, 260999, 261015,
    30893, 261053, 261067, 261074, 197393, 260659, 173509, 155240, 261082, 261093, 261101, 197397, 261109, 253953, 261131, 254149,
    254159, 155645, 261143, 261155, 261173, 141421, 261201, 261218, 66610, 261225, 74869, 141428, 187836, 261232, 261241, 261250,
    261275, 261008, 261285, 261293, 261304, 158791, 261315, 261323, 261356, 151373, 261332, 151384, 261344, 261388, 261403, 261421,
    261427, 81004, 81013, 261280, 261434, 255274, 115108, 261462, 261489, 261509, 261516, 261524, 9711, 261533, 60268, 125701,
    179041, 51246, 227106, 261546, 261569, 261595, 94992, 261581, 121131, 3107, 3125, 261615, 261624, 131424, 131367, 261635,
    261665, 261689, 261650, 167455, 261677, 36933, 261712, 46524, 190174, 261728, 261738, 160936, 261747, 242682, 146628, 242693,
    261757, 248625, 34698, 261781, 261788, 261795, 55660, 261814, 261833, 261848, 153912, 260669, 156681, 241538, 261869, 261880,
    261893, 261905, 67474, 261913, 261936, 67485, 261924, 261947, 261959, 260343, 173292, 261974, 261987, 52442, 262005, 262015,
    262023, 262031, 260680, 261896, 118533, 262049, 262064, 103964, 262079, 40624, 40634, 104011, 104020, 262087, 262096, 167308,
    140110, 167318, 91968, 98287, 107144, 196477, 246149, 262113, 262125, 90768, 190469, 21476, 21523, 209573, 258399, 262139,
    262149, 262183, 262156, 262197, 59134, 262220, 9005, 197983, 153218, 262209, 140122, 34960, 262240, 262267, 262288, 262331,
    260727, 130867, 145413, 159567, 255237, 262350, 262385, 262363, 134674, 159594, 262374, 262398, 159542, 262409, 214975, 262443,
    136444, 219274, 262417, 211637, 211645, 262432, 262464, 262491, 38606, 262511, 262501, 262521, 262540, 262553, 107933, 262565,
    262571, 262579, 242948, 262598, 10187, 262618, 262649, 262588, 242961, 262633, 262660, 260945, 262672, 37278, 262692, 262711,
    262734, 262753, 262743, 262763, 186482, 262773, 262784, 262797, 262811, 262827, 262834, 262850, 261883, 262253, 186492, 261908,
    262871, 262860, 88772, 164732, 262887, 262910, 262919, 262929, 262938, 262945, 262955, 262965, 262981, 155663, 207620, 262994,
    66170, 205506, 263007, 117339, 176581, 117401, 14248, 29254, 182313, 263020, 263035, 263013, 71196, 157784, 214892, 263048,
    84318, 126727, 210073, 126732, 263069, 263084, 96576, 128993, 179129, 252673, 263101, 99120, 263110, 155551, 209583, 68767,
    195573, 236168, 195583, 263105, 236176, 191480, 26428, 122574, 252677, 263028, 263123, 263148, 263170, 263184, 194344, 263159,
    28709, 263200, 263211, 263224, 263130, 263139, 189444, 263246, 263255, 263265, 263277, 263289, 263235, 263313, 20167, 7706,
    159620, 217187, 223191, 242028, 263328, 242039, 263340, 263352, 263363, 263375, 263385, 59988, 164741, 262879, 164748, 210589,
    263405, 263425, 87046, 263435, 263457, 196568, 263485, 263505, 119096, 119109, 263465, 263495, 263446, 263410, 119124, 94849,
    263521, 10208, 184062, 189334, 261140, 263533, 52446, 263543, 263557, 121368, 130474, 133731, 263572, 263565, 263586, 263606,
    232892, 118349, 263624, 137856, 263637, 263536, 232440, 234906, 263648, 127159, 263666, 263687, 263696, 99425, 250737, 199849,
    250751, 86749, 178376, 263705, 263715, 263735, 79307, 25460, 196694, 56308, 196705, 76339, 263725, 196715, 84747, 263768,
    63644, 79317, 51493, 25473, 263748, 24043, 230936, 263788, 15083, 15096, 15149, 42131, 169274, 263811, 163884, 230354,
    263833, 263850, 163889, 670, 696, 1675, 1660, 263866, 263898, 263876, 263887, 263822, 1761, 263912, 263919, 263904,
    108231, 261872, 263925, 95685, 96585, 49204, 187987, 247367, 230942, 230958, 187995, 157720, 263935, 15295, 263967, 263941,
    45088, 128854, 128861, 200611, 97945, 200626, 245885, 263984, 263998, 264013, 264050, 264023, 264060, 45095, 264070, 143632,
    264087, 231269, 264115, 264121, 254169, 264128, 264140, 2318, 264131, 197428, 49833, 243986, 243992, 197437, 236274, 100179,
    264149, 264160, 91816, 102536, 234994, 264198, 264206, 264224, 264216, 261149, 106123, 226717, 264240, 41362, 203159, 264248,
    177678, 242702, 203985, 256124, 264270, 264289, 264305, 264328, 264278, 264338, 82562, 264349, 264358, 264368, 264381, 264395,
    155910, 264436, 264449, 255201, 264463, 264490, 264515, 264534, 264561, 264588, 264598, 6963, 221791, 225965, 98874, 237237,
    185117, 264611, 264624, 264637, 264652, 264667, 219998, 264690, 47499, 135102, 220268, 156823, 264737, 264751, 142201, 264765,
    93984, 264779, 264793, 264816, 264706, 145609, 264843, 264876, 264906, 264929, 264918, 264952, 264962, 94232, 4317, 264974,
    264985, 264997, 100186, 5840, 223239, 30348, 265028, 265046, 265061, 172032, 223621, 175893, 4414, 223909, 265104, 265116,
    183033, 247130, 265152, 265175, 265199, 265230, 265258, 265272, 240472, 37250, 265288, 265243, 265308, 265329, 265350, 264890,
    265377, 265428, 265386, 265397, 265446, 172131, 178594, 265469, 265492, 218927, 265516, 215769, 250704, 265543, 265570, 265618,
    265627, 265639, 265661, 216166, 265684, 265703, 265724, 4428, 265529, 218663, 265777, 265796, 30357, 249735, 265822, 265834,
    29525, 265846, 226598, 265865, 265875, 265894, 265437, 189240, 223634, 228129, 215242, 265916, 265937, 265951, 265965, 265990,
    266019, 266031, 240482, 266041, 266067, 266082, 266096, 266121, 139214, 174135, 266146, 4584, 266155, 266167, 37405, 265189,
    264548, 173595, 266193, 266204, 266217, 266241, 266256, 30367, 266271, 266290, 265809, 266313, 170011, 266372, 266397, 266411,
    40741, 265907, 266426, 266455, 264720, 265213, 224662, 266499, 266512, 263842, 192981, 266549, 265165, 266569, 266581, 266598,
    228765, 41369, 118379, 193565, 266615, 266644, 266666, 266629, 266685, 266701, 266724, 266672, 266715, 266745, 266764, 266754,
    266775, 266786, 266796, 266805, 235558, 266828, 266853, 266881, 266897, 266834, 266916, 266928, 266949, 266973, 266654, 266959,
    266983, 266997, 68123, 68131, 267010, 267020, 267029, 267052, 266859, 267065, 266939, 267075, 267099, 267085, 267109, 267123,
    267136, 267158, 266678, 266843, 266887, 267175, 267187, 176633, 267199, 69351, 72985, 267208, 267218, 267243, 89093, 267224,
    244837, 90348, 244848, 267258, 166439, 267288, 59169, 90356, 267266, 267321, 140257, 267335, 111316, 174585, 174603, 267370,
    223686, 267391, 100210, 18333, 18260, 267407, 48478, 187170, 234934, 54200, 267424, 267431, 267441, 267450, 267460, 263711,
    203997, 52476, 267180, 262991, 267480, 267491, 104782, 267484, 210033, 210040, 249169, 261160, 40582, 267500, 267509, 267519,
    267528, 233352, 233333, 260956, 267538, 261721, 267549, 267557, 267564, 267578, 228238, 228260, 37817, 27088, 120967, 122380,
    29360, 261060, 120408, 267588, 267596, 267614, 249968, 267604, 267622, 259384, 239417, 259395, 267632, 267642, 267652, 267664,
    12247, 72906, 267680, 267690, 261115, 267699, 267714, 261121, 190783, 267724, 267742, 9491, 47551, 122117, 161650, 123154,
    267766, 267786, 267777, 53697, 267804, 121830, 18344, 184217, 18272, 18282, 267818, 267830, 18353, 267843, 18364, 267858,
    267795, 267872, 267812, 126739, 267192, 263382, 246945, 263550, 19117, 224501, 263795, 263803, 267892, 211554, 267906, 267913,
    267926, 267939, 263930, 260232, 260239, 103143, 67497, 267954, 267973, 268008, 204769, 77653, 2173, 9232, 128286, 182492,
    34589, 34596, 118824, 118806, 169217, 268022, 204587, 6839, 266231, 268046, 268060, 115712, 160119, 6003, 110283, 110258,
    140775, 140810, 133848, 268072, 268094, 268105, 185479, 268117, 268137, 268036, 204605, 204758, 2484, 240347, 51993, 4495,
    4507, 67845, 268163, 145044, 268201, 171055, 243779, 37168, 54803, 109146, 268211, 204687, 268221, 268240, 268252, 268262,
    268276, 149186, 268288, 268316, 268326, 6415, 76499, 259190, 76509, 259200, 116305, 128295, 268338, 216713, 216762, 268370,
    216773, 268381, 103242, 216785, 268396, 193029, 216722, 137767, 268425, 268441, 267350, 268462, 268470, 165684, 268483, 268503,
    207697, 216731, 268517, 63401, 260488, 268533, 268553, 163321, 268580, 268562, 268598, 32272, 268608, 268638, 268655, 268646,
    172964, 268671, 268686, 268692, 268476, 104477, 158832, 268698, 268708, 42071, 1650, 158838, 268719, 158844, 140672, 158886,
    172936, 268743, 268775, 268803, 268817, 268841, 172747, 127572, 239291, 268861, 268872, 156805, 125421, 127524, 268903, 127531,
    162364, 268910, 268918, 268924, 3043, 148496, 268931, 268940, 209356, 85593, 85569, 35532, 268966, 268986, 225211, 268978,
    268998, 35537, 225220, 268948, 268957, 157532, 119973, 157542, 12570, 163939, 163959, 167105, 167143, 42496, 167572, 213901,
    167609, 167627, 167654, 83043, 269023, 234498, 269034, 224888, 269058, 269046, 242710, 269077, 269106, 148507, 269126, 269134,
    171677, 269142, 171685, 269150, 220948, 269160, 269173, 269181, 188678, 269199, 78357, 228105, 269219, 269238, 269260, 70920,
    269279, 269300, 70927, 269314, 269331, 269339, 230230, 46017, 46036, 269348, 158853, 269356, 137117, 269363, 269366, 75372,
    269371, 200031, 269385, 188376, 198507, 269398, 78756, 198538, 198550, 47778, 123281, 97324, 97331, 117359, 269414, 268466,
    117362, 269426, 269431, 269417, 269440, 200469, 268731, 269457, 166625, 269466, 269476, 14533, 159286, 269470, 28829, 269489,
    269522, 269531, 78761, 198555, 188385, 198516, 18563, 18570, 269539, 248522, 248527, 269554, 269564, 186703, 269574, 269585,
    269598, 269620, 269643, 269656, 269604, 269672, 269461, 169133, 246806, 269688, 269697, 269680, 246330, 269705, 269723, 269730,
    269739, 47356, 215090, 43190, 19671, 269759, 55666, 55675, 32816, 249678, 269782, 269792, 269804, 269824, 77512, 77552,
    166629, 182422, 19706, 243455, 156996, 269838, 157009, 157024, 269851, 269865, 269875, 225708, 269885, 139530, 155827, 196192,
    269908, 269918, 269925, 269938, 257897, 229102, 229134, 229145, 269957, 269946, 269979, 269988, 269996, 58578, 237407, 56251,
    204559, 270028, 270050, 260971, 270070, 188593, 270105, 116467, 163278, 188548, 270126, 188602, 270136, 188640, 270148, 270177,
    212778, 247990, 270196, 270207, 34375, 235686, 270219, 270237, 268750, 268825, 270260, 270281, 270293, 270306, 270319, 270344,
    12110, 270359, 270377, 270406, 170987, 270439, 270451, 31374, 12119, 270368, 141340, 270386, 270477, 163024, 270395, 12148,
    236259, 270493, 77564, 4749, 146465, 172754, 268761, 268782, 268767, 268788, 270525, 13841, 270533, 268849, 136773, 270541,
    270550, 270560, 270571, 270577, 270584, 136640, 163544, 133452, 75195, 270627, 270642, 270669, 270687, 270676, 270694, 270648,
    270658, 136670, 270703, 47589, 172768, 270746, 194537, 270764, 270774, 21233, 270750, 270785, 100396, 197348, 199798, 33950,
    33964, 47555, 144421, 31302, 35753, 106216, 270803, 253399, 261979, 259574, 36781, 259645, 65017, 248703, 270553, 53380,
    270871, 204137, 270885, 270894, 114960, 270902, 270935, 270965, 35808, 271005, 271025, 54888, 271049, 271060, 270915, 270950,
    270975, 35818, 271015, 271035, 45490, 156453, 271071, 271091, 271082, 271103, 270036, 54227, 139196, 123423, 192913, 111222,
    271113, 212522, 271133, 271140, 134270, 119776, 134303, 134279, 183054, 159072, 128328, 83907, 168357, 271147, 270878, 66485,
    271169, 266732, 260495, 242507, 242513, 271184, 271190, 172193, 271200, 271223, 271259, 271235, 271246, 271272, 271284, 271159,
    271309, 271321, 233845, 271333, 233852, 271348, 66770, 271360, 271370, 271388, 271400, 124623, 271412, 153869, 78256, 234626,
    98858, 271429, 229176, 248008, 271363, 67102, 66795, 271445, 271455, 66803, 112658, 185995, 67110, 30590, 271466, 271477,
    257800, 223983, 271487, 223995, 271512, 271523, 271535, 271545, 91434, 91460, 151147, 151121, 151157, 199421, 233976, 156060,
    148683, 271558, 265735, 268935, 42274, 173648, 189309, 271611, 271669, 271673, 271678, 271695, 42340, 39840, 271736, 184620,
    204323, 271437, 271753, 61044, 158972, 172050, 184881, 233479, 271743, 271768, 271783, 87903, 187375, 87928, 114315, 271800,
    271808, 271820, 148390, 271843, 271861, 193072, 234640, 186137, 158446, 160996, 172613, 30949, 70936, 230786, 185128, 60840,
    60869, 123943, 215140, 238403, 215147, 238422, 170507, 241582, 135349, 28792, 271853, 271883, 186145, 271902, 271925, 219663,
    170640, 208298, 163978, 170601, 60851, 271949, 185148, 271968, 271976, 187657, 64706, 272004, 186727, 198582, 272020, 272051,
    271891, 148402, 271909, 271792, 171981, 61050, 159996, 160074, 17241, 160079, 110355, 272102, 272153, 213405, 272181, 272189,
    66032, 272212, 43005, 146073, 150787, 197493, 217216, 272240, 103886, 272260, 272269, 272281, 271871, 198604, 272311, 272320,
    272332, 105433, 213450, 184478, 168971, 136473, 272361, 272382, 14227, 195185, 238229, 272220, 272399, 272229, 204447, 272417,
    272435, 231087, 272454, 237070, 215619, 272475, 272503, 272483, 272519, 272511, 272251, 272292, 272464, 272534, 109055, 244006,
    272562, 121794, 272116, 272125, 130479, 263778, 183654, 160448, 272585, 272611, 272630, 272652, 272674, 223866, 272391, 272747,
    3560, 272755, 272764, 272408, 271985, 271994, 168980, 272788, 157790, 195144, 272426, 272817, 169399, 272837, 10097, 182071,
    153843, 115151, 272598, 148829, 174534, 160085, 207180, 160093, 272856, 272032, 210897, 16334, 272868, 272884, 272901, 272919,
    272944, 272931, 132354, 272958, 89199, 206514, 272973, 272999, 272981, 273008, 272990, 273017, 60479, 213123, 273030, 273045,
    273059, 131652, 213190, 273085, 211960, 273102, 273112, 130186, 273122, 273132, 273148, 108769, 184728, 273178, 273188, 273198,
    273208, 273214, 273225, 273237, 273266, 273283, 273302, 81635, 273314, 273329, 273341, 273354, 144698, 108886, 111516, 144709,
    152072, 152081, 69855, 273372, 230885, 45930, 46785, 246588, 273387, 204841, 45901, 21407, 128141, 184577, 128159, 105969,
    237518, 154375, 83699, 253865, 273403, 273414, 21414, 273424, 273431, 273446, 273465, 273488, 273509, 273547, 273568, 167927,
    192384, 206079, 273581, 273594, 273586, 54018, 273606, 273614, 189636, 265932, 121735, 273023, 75344, 273622, 273627, 273635,
    267573, 224897, 2982, 13988, 229759, 149009, 23660, 114034, 114075, 64878, 273655, 88345, 273669, 117775, 121149, 191898,
    249823, 270486, 273686, 230900, 273699, 74448, 87997, 93050, 273713, 273760, 273785, 186014, 273818, 273837, 273847, 130571,
    273857, 273884, 273870, 273904, 273936, 273912, 273924, 273953, 273944, 273970, 54979, 273989, 273998, 274010, 274026, 273729,
    7660, 63559, 274038, 33984, 273744, 274057, 274070, 273773, 273802, 186030, 274084, 82787, 273828, 274107, 274122, 91710,
    127904, 274139, 40199, 52888, 274115, 274130, 273645, 18904, 18996, 274003, 274015, 274047, 7669, 274098, 119495, 7680,
    99247, 202949, 274152, 34504, 34514, 274164, 97363, 97371, 67394, 243687, 274177, 274196, 30552, 274233, 176658, 172918,
    274251, 183576, 274273, 274261, 273979, 274296, 250847, 262475, 267150, 105671, 89101, 131906, 7286, 65190, 274317, 274326,
    274335, 51688, 274363, 274370, 274377, 152164, 274406, 274416, 7864, 274428, 162038, 162048, 168232, 196447, 274452, 274433,
    274471, 274442, 274480, 274344, 274353, 245730, 274490, 274498, 27104, 187444, 187453, 274505, 142965, 274521, 274529, 35362,
    35873, 61980, 274553, 274562, 129242, 274573, 173515, 254588, 274588, 72381, 274602, 274612, 88353, 230689, 274624, 274644,
    254805, 254816, 274663, 274673, 78809, 78618, 64002, 274631, 254594, 247374, 130189, 274685, 30247, 274706, 233461, 274729,
    274692, 263515, 274742, 274758, 274780, 274768, 274799, 274822, 274830, 94514, 94524, 274842, 274852, 274862, 274869, 274878,
    274918, 274935, 130194, 274926, 274957, 55043, 274982, 274999, 275009, 99497, 275021, 275032, 170210, 275044, 248422, 275067,
    275078, 275091, 157728, 275101, 275112, 18531, 30375, 275125, 23710, 237825, 275181, 275196, 119030, 275213, 275228, 275245,
    275267, 275255, 275277, 275287, 275297, 275308, 275318, 272685, 111689, 275330, 275340, 172238, 72116, 31960, 275134, 239113,
    63889, 165106, 275349, 63899, 275359, 27437, 275371, 274987, 275388, 273380, 275414, 11652, 236465, 107026, 169885, 275427,
    275437, 249902, 275450, 275462, 275476, 275498, 275511, 275518, 275539, 275549, 275528, 275558, 275569, 275581, 275590, 87911,
    275607, 213475, 275619, 47689, 109482, 275641, 26673, 275646, 96919, 275655, 275670, 275662, 275681, 275693, 275721, 275731,
    240516, 226355, 236632, 275740, 275143, 275760, 275777, 275789, 275152, 275162, 275171, 275799, 275820, 275834, 275841, 275847,
    250763, 275870, 250780, 275887, 127351, 275904, 275931, 14466, 184156, 275947, 8774, 40560, 167214, 179331, 275966, 275986,
    276016, 276001, 154029, 164500, 276042, 149505, 276060, 164509, 276051, 107034, 276081, 65365, 276096, 69926, 23997, 275505,
    36019, 276114, 276131, 276122, 276139, 276156, 276162, 271353, 273125, 276173, 273231, 276192, 276201, 130671, 9859, 276224,
    276232, 28801, 28807, 276243, 276266, 276282, 276289, 271420, 127831, 165167, 205662, 161172, 218687, 276298, 276315, 276327,
    276337, 276361, 195057, 195069, 276385, 276401, 276413, 276425, 276432, 189771, 127358, 33319, 75516, 156573, 207319, 74670,
    156583, 198437, 238628, 238636, 195111, 276395, 99201, 129994, 194359, 273437, 276448, 276481, 273246, 273256, 276504, 234873,
    146776, 267362, 30561, 276514, 15499, 249911, 121273, 230307, 276209, 203211, 276538, 276564, 276590, 276607, 223780, 146141,
    276662, 146152, 276673, 141186, 276686, 184593, 276703, 184839, 276717, 276726, 276735, 276744, 270420, 276758, 276765, 276528,
    35019, 35057, 15515, 276772, 276784, 24328, 261607, 94957, 244347, 261558, 15524, 276796, 276806, 181255, 187136, 276818,
    276833, 276852, 33421, 276863, 33433, 276874, 276843, 99273, 103185, 139395, 80316, 132865, 276911, 55569, 2255, 2221,
    276921, 276937, 249073, 276930, 276955, 276962, 276970, 276976, 202687, 102490, 226163, 274699, 274735, 275771, 107434, 276983,
    109991, 189641, 86302, 277001, 277017, 277033, 277058, 277020, 253980, 86145, 277070, 274149, 271045, 277088, 273362, 277100,
    276306, 277110, 111525, 152045, 32022, 53317, 71681, 277119, 16205, 277142, 156357, 225784, 52301, 66759, 229153, 277154,
    277164, 98681, 49015, 75533, 277174, 277181, 198001, 277187, 277036, 277201, 277210, 52517, 277218, 52450, 141802, 277229,
    277236, 122585, 277258, 277263, 277283, 277272, 96120, 134593, 127576, 275977, 127539, 148625, 257241, 158002, 277301, 277311,
    158010, 58198, 277323, 277337, 277345, 67356, 4923, 204849, 273456, 273477, 184703, 277354, 156267, 184498, 166726, 277369,
    273499, 273523, 273536, 277400, 277408, 63751, 277416, 277425, 277470, 204857, 273558, 277488, 277508, 180789, 275432, 156875,
    139995, 277543, 172863, 277557, 109945, 236285, 249745, 26436, 122593, 58929, 277576, 277602, 237260, 277583, 239361, 264244,
    267981, 239365, 211654, 54867, 277621, 147156, 179867, 277640, 277645, 54875, 277654, 277664, 104845, 268663, 60637, 236499,
    236509, 54985, 126438, 241395, 101704, 241426, 241438, 118551, 277674, 277686, 66440, 188883, 208227, 103845, 277699, 131774,
    277712, 208877, 277725, 277733, 277742, 277759, 277771, 277788, 277797, 93244, 277807, 93252, 277815, 91824, 277823, 277833,
    277843, 142509, 277870, 9095, 277890, 9107, 277902, 277914, 127210, 277921, 251948, 277928, 277947, 277746, 277959, 277972,
    277996, 278014, 216641, 278022, 278043, 278057, 278067, 273135, 278001, 278088, 274537, 278079, 208945, 278109, 278120, 74338,
    278130, 115400, 182679, 278146, 278172, 100126, 278201, 278227, 278181, 253589, 278191, 278238, 278250, 278269, 278281, 262720,
    278296, 278317, 253612, 274545, 216651, 275860, 153918, 273575, 193617, 174498, 278340, 278347, 278354, 84765, 278361, 278368,
    211457, 193621, 69006, 278379, 86961, 31647, 278409, 54812, 140279, 18163, 278426, 278441, 225692, 278458, 278494, 54822,
    278518, 278526, 179002, 238654, 278506, 278537, 278562, 211477, 278614, 278625, 155181, 256132, 43388, 278636, 277631, 34296,
    278665, 278694, 278709, 278727, 277880, 278747, 278786, 278757, 167266, 269006, 278802, 127839, 32980, 129572, 97723, 278819,
    278831, 246226, 97732, 97741, 275701, 278843, 100495, 248548, 278863, 268494, 278884, 278893, 129140, 278904, 278916, 35105,
    35143, 278928, 269898, 187673, 21025, 278873, 278852, 278950, 278979, 278964, 279000, 278217, 279028, 279043, 279062, 34060,
    77354, 256928, 279080, 279095, 279106, 237646, 279085, 279117, 279130, 279144, 142519, 179012, 227485, 279153, 279164, 19383,
    279185, 279210, 279255, 279224, 279279, 48269, 225752, 279297, 279321, 279344, 126888, 12681, 89984, 279367, 279391, 112896,
    279412, 279445, 117897, 150106, 143351, 229032, 229053, 229069, 115899, 150119, 150131, 150050, 204717, 237664, 279476, 150186,
    58479, 279332, 222181, 215467, 222277, 222290, 244559, 279510, 232161, 279542, 241404, 279554, 279580, 279605, 117105, 222551,
    117117, 217271, 150061, 150143, 31397, 34070, 183513, 183536, 148925, 279036, 279054, 142528, 279175, 116315, 138572, 116272,
    279623, 116285, 279636, 119356, 135670, 138581, 148780, 234519, 277857, 184463, 141484, 279647, 279656, 63021, 26388, 63026,
    238610, 119367, 279665, 48280, 279677, 279687, 259263, 131762, 176130, 277025, 277045, 277061, 279708, 279716, 29547, 279725,
    239768, 238813, 48705, 143531, 279734, 279776, 279792, 279801, 239778, 191030, 269375, 213522, 279810, 184172, 279828, 89782,
    279849, 242408, 279869, 122601, 58941, 178385, 244281, 122609, 277590, 61087, 237267, 277596, 252604, 252621, 273964, 278743,
    279910, 279932, 46411, 270327, 270270, 174045, 279947, 251954, 279963, 30628, 30637, 279988, 280003, 30702, 30647, 280041,
    280051, 280099, 280112, 30292, 280127, 280141, 153969, 52289, 280157, 280172, 280194, 280206, 280184, 280215, 118242, 7425,
    7294, 280232, 277074, 261410, 280257, 280280, 244799, 244813, 213134, 280299, 280308, 280319, 280351, 280362, 254774, 280375,
    7342, 22109, 78433, 100065, 143152, 23091, 280224, 280396, 56174, 20357, 280428, 280439, 197026, 197063, 197038, 197075,
    178837, 280452, 244208, 280474, 280494, 280506, 280517, 280529, 280543, 280557, 280571, 280579, 280587, 74613, 120441, 280604,
    181310, 280624, 280638, 280649, 50900, 100133, 30024, 280289, 156188, 280662, 81413, 280678, 50926, 147950, 280687, 280694,
    280705, 50933, 147957, 280718, 280728, 271299, 280739, 280750, 132380, 171399, 280762, 280778, 280796, 280810, 202320, 181087,
    182658, 171405, 239807, 198270, 280826, 280838, 280852, 280867, 258287, 280880, 280892, 186767, 122506, 280904, 280914, 280923,
    280484, 101166, 101177, 280945, 280972, 280989, 280932, 29498, 184957, 25772, 118702, 149810, 120446, 281004, 281016, 24206,
    75920, 68492, 280959, 281030, 281042, 281055, 270158, 281068, 281079, 281085, 281091, 235417, 281102, 235425, 281110, 281118,
    277054, 211045, 277195, 171416, 160296, 281131, 281163, 281172, 178391, 281141, 97693, 97704, 281152, 281180, 281187, 98514,
    281195, 243799, 274031, 281215, 281242, 128650, 87418, 193059, 179342, 281253, 254085, 254095, 197741, 281263, 281273, 171420,
    214247, 273678, 138238, 165005, 277512, 178400, 281282, 281301, 281310, 281319, 281336, 278768, 9119, 128763, 281356, 280063,
    281401, 280073, 281411, 281423, 178409, 76315, 200001, 209385, 281439, 281455, 126613, 209415, 209431, 198763, 209439, 281477,
    76474, 76520, 281483, 281502, 273661, 281519, 281531, 281538, 281549, 281558, 112224, 171558, 281583, 281589, 281608, 281596,
    281602, 281629, 281646, 281664, 26792, 244415, 139849, 243895, 139907, 108246, 108260, 246699, 281683, 150558, 281704, 281615,
    202140, 44274, 121626, 261842, 281716, 281622, 281738, 281751, 281760, 202042, 274965, 281768, 274973, 281776, 274386, 8018,
    8026, 232489, 95692, 281785, 124070, 208820, 281800, 9717, 79769, 216661, 281826, 23928, 264078, 281842, 67148, 278050,
    281863, 281876, 281891, 281904, 281915, 281896, 281909, 281922, 42258, 281928, 128668, 281942, 281950, 281959, 281970, 281934,
    232498, 281979, 281992, 282004, 282014, 91856, 282029, 281985, 199607, 281429, 282048, 277564, 282055, 282067, 282081, 262231,
    241548, 46917, 282091, 282113, 282121, 253686, 282130, 282136, 282072, 282060, 282144, 282154, 237497, 282104, 277570, 282166,
    282196, 282175, 282205, 163339, 282246, 282260, 282184, 282214, 86326, 86333, 248250, 75210, 109026, 282289, 282331, 154599,
    282338, 176726, 282347, 282356, 282367, 179881, 282380, 282392, 179836, 26947, 277519, 26443, 282402, 86731, 202882, 282418,
    282433, 168877, 212565, 282455, 282463, 185539, 204864, 231333, 282474, 203220, 282503, 282518, 282532, 203228, 276547, 138321,
    203237, 276556, 282550, 282590, 282606, 107682, 273094, 282632, 9924, 20544, 214069, 109926, 273693, 282646, 282659, 282673,
    277668, 74646, 282681, 33477, 74652, 282687, 155833, 155840, 30831, 30841, 30851, 282698, 242992, 282734, 282763, 35789,
    168379, 282772, 165801, 282784, 282810, 167773, 282861, 282879, 282296, 282308, 282891, 282904, 282319, 269631, 279700, 282916,
    209869, 279570, 64716, 110178, 258619, 70592, 282929, 282941, 282950, 181409, 181389, 282960, 282977, 166529, 194572, 282986,
    282996, 283018, 226643, 183245, 11219, 14314, 116432, 160506, 182836, 283090, 199806, 217485, 211246, 71323, 213160, 283102,
    283115, 283158, 283164, 178123, 56631, 88420, 132098, 283171, 283191, 283208, 283179, 178235, 185692, 283226, 283241, 247242,
    283253, 283265, 137931, 283281, 283296, 283314, 283287, 283332, 283354, 282869, 98584, 283375, 74126, 74139, 47148, 144893,
    181426, 154395, 73806, 73817, 73830, 283387, 73842, 283399, 73856, 283413, 283426, 13820, 142080, 243883, 49728, 118209,
    8901, 226193, 275959, 283440, 188527, 283460, 283451, 277763, 283493, 152894, 247208, 281125, 70610, 283108, 12403, 56948,
    283507, 163574, 282935, 283530, 283544, 79883, 283549, 283558, 283583, 283604, 283613, 283622, 283635, 283649, 22972, 283659,
    22982, 69325, 283673, 283687, 283698, 283705, 92061, 283731, 283776, 283794, 283805, 283818, 278547, 160457, 272166, 283841,
    283851, 283830, 278555, 283862, 55962, 283874, 55982, 154440, 159173, 178251, 283885, 283915, 55990, 154448, 159181, 283234,
    283893, 283923, 183869, 283536, 124054, 283945, 283957, 184712, 184644, 283971, 283987, 284011, 238691, 183251, 183257, 14322,
    105514, 248199, 284038, 144074, 284054, 284066, 284084, 175033, 284100, 284125, 284153, 280466, 284061, 284076, 284093, 284110,
    175044, 284117, 284137, 284166, 200817, 176704, 200834, 276459, 63428, 67581, 176288, 284177, 284200, 175850, 197766, 284214,
    284222, 176296, 284231, 123825, 284185, 284243, 284256, 284272, 284281, 284206, 284290, 284192, 99189, 183262, 284298, 160640,
    176321, 284250, 284266, 139812, 243904, 139916, 149285, 152027, 154818, 216045, 149329, 282425, 284304, 149292, 183268, 51600,
    38455, 238198, 238204, 144427, 284323, 284335, 51282, 201294, 229777, 284349, 284362, 284383, 283951, 283964, 284392, 281745,
    284400, 284411, 161871, 284425, 284445, 284465, 284494, 197224, 65992, 249986, 284506, 284516, 284534, 284561, 284574, 284591,
    59178, 202699, 284523, 284541, 105637, 284604, 154886, 97393, 108342, 108359, 21363, 284616, 284623, 284630, 284650, 284657,
    284666, 284675, 276693, 13564, 157404, 157443, 157458, 284711, 198109, 127815, 284724, 284736, 195610, 271618, 284750, 219024,
    198117, 29834, 107463, 284770, 284786, 284805, 284832, 102679, 143224, 231251, 102685, 143230, 231257, 284849, 284873, 34214,
    141349, 282667, 284887, 284911, 284928, 284899, 284941, 278432, 284954, 284964, 284978, 284985, 180090, 284993, 285007, 285025,
    285015, 285033, 285044, 284919, 269189, 77876, 285053, 153157, 254636, 285075, 100999, 101111, 263595, 97640, 61826, 284853,
    284864, 276509, 285099, 285112, 285104, 285128, 285141, 285120, 84073, 115333, 135332, 95491, 275810, 285150, 284497, 284880,
    122890, 260604, 285167, 285178, 285189, 285200, 285213, 285224, 285234, 285248, 285259, 22937, 285271, 285282, 285296, 285321,
    207919, 241693, 285344, 283718, 285356, 22569, 259339, 285308, 207930, 285369, 259361, 116199, 285392, 285403, 97522, 178793,
    285415, 8222, 285427, 285377, 285441, 285454, 285465, 14745, 285493, 285334, 285524, 285536, 259456, 75002, 75019, 285549,
    285559, 285566, 285574, 285598, 58347, 235015, 285614, 285553, 157075, 285631, 224012, 91759, 197503, 224022, 285649, 274391,
    285661, 106021, 259237, 74346, 206271, 244762, 205758, 206283, 149225, 118575, 65608, 285655, 285671, 30253, 165647, 197511,
    285684, 285691, 231485, 49626, 173521, 285666, 285698, 122669, 263828, 285710, 203245, 218771, 16212, 170400, 173526, 76176,
    130200, 285723, 285735, 285679, 270062, 274889, 243385, 137009, 285727, 285744, 285756, 285790, 209445, 206041, 285800, 285819,
    275397, 285830, 285795, 11371, 285848, 209501, 285841, 285857, 160726, 227083, 237282, 270044, 175765, 197522, 285868, 151129,
    160731, 22777, 206727, 285890, 193994, 270313, 285903, 241324, 285912, 138247, 241336, 285638, 245034, 16573, 238495, 256273,
    285922, 285928, 101889, 180228, 41415, 238500, 18178, 11928, 220552, 285935, 285970, 285948, 285994, 286018, 286005, 11941,
    220565, 285959, 285983, 28286, 137142, 268230, 286041, 192268, 272874, 286062, 270337, 270352, 286081, 286092, 246571, 246603,
    274810, 237704, 286105, 196988, 173979, 202159, 286145, 286155, 87030, 186661, 286166, 286177, 286191, 286202, 286213, 286234,
    286223, 127040, 44893, 41424, 52916, 69359, 57439, 153389, 286251, 152599, 259752, 2229, 118750, 286266, 286285, 286299,
    42755, 158253, 286317, 286333, 142036, 241099, 286071, 286347, 99226, 284552, 286365, 286411, 286292, 286258, 91592, 257394,
    286440, 286454, 102027, 234486, 82746, 92910, 161692, 286472, 286308, 284568, 284581, 77984, 173532, 204331, 204336, 96339,
    286491, 88428, 186371, 286501, 286512, 277092, 259270, 133507, 286525, 200710, 158028, 211072, 281226, 200663, 286558, 286593,
    179196, 286602, 258456, 71629, 207360, 286324, 179399, 217782, 286632, 277719, 71429, 286655, 286671, 169914, 270114, 286686,
    203524, 286702, 9738, 286720, 286740, 286730, 286751, 286764, 286779, 286790, 286821, 286842, 286863, 203534, 286881, 286891,
    286903, 286868, 286875, 237950, 237923, 31080, 237960, 86974, 286922, 33487, 286957, 209210, 286658, 45256, 194075, 286992,
    287002, 287014, 287027, 255082, 89037, 287045, 287076, 275710, 287098, 287115, 287135, 287176, 287186, 287196, 287209, 27149,
    27160, 276366, 83970, 150826, 287223, 96068, 128459, 287247, 120137, 228336, 287234, 3671, 3684, 204457, 204469, 3697,
    152195, 204377, 3596, 204481, 3713, 3751, 6754, 102112, 287263, 287275, 287285, 268451, 101213, 185305, 287312, 287335,
    287357, 287346, 287368, 248258, 248285, 287379, 287389, 140459, 185254, 140474, 185267, 287448, 287469, 185002, 277379, 136678,
    287507, 150668, 272695, 272707, 287520, 287534, 238724, 284237, 287549, 287561, 227966, 227978, 287573, 176134, 145984, 176144,
    287584, 11626, 186796, 250671, 104279, 44053, 287594, 287606, 146012, 130833, 287620, 130845, 287632, 34132, 265504, 277392,
    27362, 287645, 287664, 287542, 93200, 284403, 287683, 287700, 287691, 255968, 58170, 287323, 27307, 41517, 31738, 27373,
    55883, 204696, 287714, 287726, 286243, 185813, 287736, 287748, 147631, 287773, 272719, 272732, 4596, 287798, 287810, 185367,
    279423, 175214, 234332, 31311, 269208, 266815, 52964, 187858, 147595, 57177, 287823, 267468, 114535, 287841, 114544, 287462,
    92480, 287881, 103590, 110747, 229192, 71437, 287897, 119135, 287913, 287930, 113762, 167501, 287971, 39052, 39017, 39064,
    287297, 287090, 288004, 288012, 200335, 288024, 38982, 147963, 173908, 200344, 288033, 237339, 288042, 271500, 224034, 237350,
    265748, 288053, 200353, 286772, 286801, 286811, 286832, 286853, 128477, 288075, 237004, 150680, 287787, 150691, 166069, 240531,
    28107, 116118, 162280, 288092, 288116, 116131, 162293, 288105, 288129, 185720, 269480, 287655, 287891, 288153, 288165, 279955,
    120070, 120081, 228311, 228349, 120157, 70490, 288184, 288192, 288229, 73460, 73501, 117825, 120164, 146085, 172703, 288253,
    208382, 288264, 288273, 288294, 288371, 269750, 254907, 288303, 238743, 3143, 3153, 85765, 50248, 96463, 34140, 98129,
    252471, 287708, 288393, 180060, 189181, 288409, 3160, 260451, 189188, 288416, 33729, 260842, 199960, 288384, 288434, 3169,
    26128, 288458, 288467, 288475, 136781, 129493, 96471, 132837, 288498, 288485, 22948, 242148, 287922, 27393, 2066, 27171,
    231565, 85042, 130488, 288308, 288513, 222221, 288543, 288553, 288521, 125389, 279435, 225046, 261185, 51365, 230795, 288567,
    98135, 231571, 288582, 193971, 93067, 288595, 93148, 241116, 288612, 275748, 288660, 288670, 225365, 46667, 133048, 46679,
    133060, 288680, 288693, 288706, 167995, 263677, 288722, 288744, 10244, 115880, 30959, 248391, 288768, 288782, 121865, 26352,
    120256, 143191, 127050, 192045, 257365, 193040, 282746, 274397, 288800, 233686, 142692, 195539, 219835, 267544, 172323, 288817,
    172288, 108073, 169142, 285879, 288848, 288858, 248171, 252281, 288868, 271932, 288876, 175686, 192081, 288887, 288902, 288915,
    288933, 62069, 62089, 288948, 288964, 288999, 289017, 44386, 288599, 288938, 9748, 289046, 189621, 189674, 11952, 189688,
    172300, 63574, 133140, 172332, 288826, 188699, 289067, 289107, 134548, 289125, 134561, 289138, 172642, 118591, 118603, 288447,
    230240, 289149, 289159, 213975, 213986, 287480, 287491, 185012, 105694, 11722, 240563, 289171, 289193, 289209, 48586, 184885,
    154210, 289223, 289235, 232933, 255725, 289247, 232943, 288733, 288756, 257182, 245185, 160099, 289265, 289271, 176437, 177395,
    177436, 115846, 171865, 160207, 171801, 247219, 275051, 280267, 284414, 76126, 272063, 289290, 289318, 210941, 289327, 185194,
    67589, 217989, 216017, 43407, 289336, 289348, 289359, 289382, 289404, 289420, 58406, 289440, 289430, 58414, 248718, 16587,
    289454, 289468, 287760, 289479, 289492, 147986, 289507, 289532, 221652, 7384, 216802, 289558, 289580, 289569, 289591, 289624,
    289602, 289634, 8319, 289644, 8358, 8383, 32160, 118880, 289459, 270591, 270602, 289662, 289672, 28323, 89593, 111969,
    288140, 289684, 289516, 289706, 171106, 289719, 42409, 289732, 12355, 143456, 289750, 289777, 289797, 289761, 204056, 258861,
    289819, 131877, 272890, 118891, 289544, 289613, 289652, 109441, 156278, 289838, 32044, 184896, 184976, 289860, 289871, 289850,
    167477, 83131, 67400, 67411, 108865, 176092, 202230, 175169, 289882, 289891, 289901, 289911, 262899, 51952, 289410, 289921,
    156286, 273151, 10532, 15681, 278389, 289945, 134910, 278398, 115721, 137341, 289961, 289980, 289988, 273164, 201380, 206973,
    289998, 163075, 33900, 33911, 33922, 290020, 290030, 290050, 290040, 290060, 171811, 290070, 290083, 115859, 76180, 232954,
    290095, 290107, 76137, 76148, 55522, 290118, 243203, 137237, 137292, 239037, 290138, 290145, 60012, 109274, 176615, 60023,
    17286, 251377, 255306, 290152, 290170, 75541, 288625, 288637, 290185, 109284, 14770, 179504, 15227, 69621, 290225, 290247,
    289696, 290252, 282757, 183477, 290260, 199969, 288315, 199977, 102823, 243931, 290274, 244512, 288084, 125522, 231577, 267229,
    269712, 173918, 173924, 290289, 290309, 279011, 290330, 290340, 289930, 95752, 95769, 290353, 290375, 290384, 290395, 290406,
    290417, 290430, 119156, 290445, 290457, 290469, 290483, 290503, 290511, 290521, 290538, 23817, 290564, 290579, 290596, 290610,
    263298, 290622, 290643, 290656, 290673, 88986, 290528, 290551, 290692, 290699, 96104, 181807, 283384, 250050, 290707, 290722,
    290741, 290749, 284359, 286420, 152346, 243364, 152357, 290758, 290780, 290791, 290769, 290801, 289394, 63770, 290812, 105283,
    191357, 290824, 33496, 217440, 290836, 290840, 9589, 258995, 285703, 99138, 154892, 10768, 96669, 156247, 64173, 64200,
    64180, 64207, 224050, 68830, 200783, 228162, 69957, 32178, 14995, 49370, 76759, 290818, 290848, 290856, 290865, 152725,
    187395, 290882, 286530, 284475, 284485, 182059, 259874, 259889, 21370, 290900, 290914, 290934, 290922, 72328, 286565, 286610,
    286579, 286621, 88361, 58354, 285581, 205524, 290949, 278796, 290962, 89263, 290968, 253546, 253504, 289370, 278776, 224752,
    60344, 8166, 49212, 87251, 278676, 279879, 290976, 277777, 34383, 291007, 291016, 274513, 76719, 196347, 234353, 291025,
    291032, 291039, 284959, 57285, 291045, 291064, 291074, 157951, 291084, 123444, 248659, 291068, 291102, 291111, 47449, 268432,
    72820, 240277, 78938, 291121, 291136, 277435, 291173, 277445, 291183, 291205, 210651, 29505, 88966, 291218, 291230, 102062,
    291238, 252236, 290266, 291248, 288323, 291258, 210660, 157687, 164986, 284717, 161537, 217003, 291270, 291288, 291277, 291295,
    140410, 188565, 271628, 16314, 202710, 291311, 184045, 94606, 94621, 64340, 177537, 291325, 210978, 177547, 87229, 86979,
    291340, 77064, 291354, 276216, 291348, 291372, 291404, 291386, 127100, 234679, 291426, 291434, 127105, 234684, 291334, 234690,
    123567, 8033, 238509, 291452, 119332, 291465, 133542, 291478, 133554, 257401, 286447, 286463, 141809, 265886, 112907, 163525,
    291501, 235217, 291516, 291509, 281525, 230907, 273706, 162737, 291525, 179693, 179703, 291534, 291544, 291556, 291566, 291576,
    291588, 280248, 9886, 9896, 9909, 291605, 291618, 291597, 291633, 214769, 291645, 214779, 291655, 291666, 291685, 291698,
    179739, 291709, 291721, 291735, 291745, 291756, 291767, 291778, 280596, 291785, 282677, 291795, 282692, 291804, 79273, 189505,
    189513, 233161, 88140, 233370, 233383, 291810, 291819, 253353, 291826, 291848, 291864, 291880, 291896, 291853, 291907, 291941,
    291954, 291869, 291918, 291966, 291979, 291991, 292001, 186288, 186307, 186204, 291885, 186317, 186328, 255331, 65861, 255336,
    292013, 292021, 124171, 274283, 292028, 187357, 292041, 246835, 91924, 138219, 292057, 81186, 98141, 292078, 82217, 82234,
    292105, 292123, 292114, 292132, 292150, 250868, 22523, 292158, 292141, 292168, 292178, 115641, 292189, 288158, 288170, 292198,
    186349, 285715, 186357, 186215, 59778, 1134, 126381, 60499, 88144, 186223, 1119, 186231, 292208, 292219, 186242, 186253,
    292230, 292250, 29262, 242720, 261766, 233598, 233614, 173537, 234386, 292285, 292297, 292307, 127585, 173543, 210421, 125428,
    292315, 292323, 292342, 292368, 37692, 187186, 292392, 151754, 112939, 37700, 151859, 225295, 292416, 99740, 89794, 163823,
    82026, 90428, 166509, 208193, 95054, 78400, 292438, 80493, 281870, 108829, 292462, 39779, 292484, 292498, 292513, 292522,
    56613, 292532, 100140, 292467, 90893, 167823, 100146, 180214, 100154, 292473, 125438, 125444, 234506, 106959, 138049, 182231,
    274289, 290942, 292540, 169035, 256226, 292428, 106403, 118391, 166637, 292558, 168667, 292578, 292588, 292594, 292608, 292622,
    292631, 112240, 203868, 237154, 292642, 174546, 246066, 246075, 292663, 16653, 204410, 292678, 292691, 292701, 48519, 292712,
    292683, 292722, 292730, 45738, 292751, 292768, 292781, 292793, 60353, 190225, 27925, 292803, 292810, 292820, 292831, 292842,
    264258, 292857, 292876, 24781, 292671, 292849, 190182, 242527, 292894, 292911, 292922, 292942, 292963, 86248, 292977, 292915,
    158488, 194967, 158497, 52576, 292984, 50262, 172549, 199769, 293001, 293023, 293044, 293059, 293069, 292928, 293111, 293138,
    293164, 293192, 293203, 293219, 293234, 293149, 293176, 293123, 130380, 292581, 293250, 293263, 232657, 293272, 131373, 293286,
    47477, 293301, 293308, 293316, 293325, 293346, 117933, 293372, 117939, 25542, 211432, 154345, 21459, 56531, 56539, 78732,
    139926, 248295, 293393, 293406, 293424, 85101, 293398, 293435, 293452, 293411, 293444, 293463, 293483, 35849, 103696, 277707,
    293497, 293511, 293526, 293502, 186071, 293546, 293516, 293580, 293594, 293603, 73946, 159817, 15912, 267750, 293613, 293618,
    51459, 293624, 293632, 63585, 198451, 293639, 63590, 131381, 293294, 131434, 293650, 293657, 293665, 260020, 70965, 293697,
    143056, 250623, 293741, 293780, 66091, 293789, 293798, 293810, 293818, 293827, 293835, 293804, 70900, 20073, 40259, 70972,
    70979, 219899, 293851, 76674, 293844, 53764, 293867, 293888, 293912, 293923, 164856, 203165, 35641, 293934, 109076, 293945,
    239616, 190440, 293940, 293960, 3948, 126118, 196923, 293978, 177448, 177736, 108426, 243914, 293995, 294013, 293875, 234757,
    157572, 287399, 294026, 157636, 198786, 157650, 108382, 139820, 139858, 294055, 294068, 293279, 294081, 90438, 90447, 294094,
    294061, 3817, 3833, 151202, 216380, 234767, 157273, 287409, 157278, 294106, 294129, 159782, 294111, 224392, 159791, 294120,
    216604, 127551, 224401, 294143, 216392, 234779, 286970, 294158, 61220, 140750, 184341, 184303, 206225, 294203, 197949, 197957,
    294215, 294221, 57241, 61294, 76764, 294233, 294250, 294258, 294266, 292737, 292743, 273183, 294241, 294279, 162855, 294299,
    226724, 286029, 294321, 294333, 294346, 294353, 36164, 199047, 294272, 30169, 125341, 294361, 183546, 294375, 294409, 294417,
    17713, 174750, 294429, 294448, 294462, 294479, 104710, 54321, 240355, 294515, 294525, 139864, 235063, 294535, 294549, 294558,
    157582, 287419, 294036, 157591, 167709, 294020, 294568, 294577, 84596, 53323, 133197, 52539, 52548, 267899, 137085, 294587,
    294605, 166749, 292591, 294617, 294628, 31820, 294644, 294668, 294656, 294698, 239398, 134008, 155975, 173328, 6299, 286914,
    261702, 6243, 6308, 234426, 232357, 294718, 194334, 204265, 45946, 166167, 294737, 267758, 293859, 45864, 294742, 294751,
    173718, 294768, 294784, 256481, 294804, 294312, 226745, 89486, 254939, 237435, 199095, 294842, 292615, 292601, 294874, 294882,
    199099, 226444, 250195, 144563, 135079, 253203, 294890, 68940, 157910, 294921, 112981, 294959, 294973, 186401, 295019, 295033,
    295050, 228715, 295081, 242097, 295099, 295118, 237691, 295126, 91328, 270079, 270093, 118355, 224961, 263630, 295137, 118362,
    295153, 295166, 295176, 171620, 45962, 295192, 279290, 295108, 260146, 295212, 295226, 184346, 206118, 184312, 80890, 295240,
    295272, 295290, 244106, 207526, 295312, 103317, 295320, 282708, 5437, 295340, 282721, 5448, 295351, 68948, 124177, 234042,
    243506, 276346, 111536, 5978, 4472, 4517, 295362, 144901, 295392, 295411, 127456, 295434, 116606, 30176, 122899, 122915,
    295458, 20822, 295476, 295490, 163774, 271211, 268148, 295504, 295526, 295538, 295550, 127365, 275914, 295298, 6659, 295572,
    278646, 295597, 295616, 295626, 148042, 264807, 295252, 295635, 295642, 295668, 85781, 179437, 295688, 96549, 242876, 2764,
    124114, 86474, 292949, 87688, 295704, 294004, 98807, 295726, 295754, 295736, 295767, 295778, 295789, 87080, 87088, 16694,
    122005, 69392, 35564, 168832, 295806, 185764, 63778, 224872, 295824, 295833, 134399, 295841, 295849, 295859, 295870, 169308,
    283742, 295881, 157046, 65302, 295897, 76603, 295906, 295912, 293893, 231832, 295925, 295952, 65282, 85954, 201061, 295967,
    295993, 37359, 295997, 296005, 159666, 285084, 288792, 101032, 287851, 296022, 296047, 296081, 74356, 296102, 296034, 296058,
    296117, 43619, 296071, 296092, 43626, 295715, 296130, 296142, 125865, 125874, 289183, 296154, 135444, 296165, 296176, 296191,
    294987, 186414, 296204, 228548, 70394, 70405, 267413, 296224, 296236, 214191, 39641, 295420, 296250, 158506, 158517, 52584,
    190743, 112952, 112990, 217010, 35857, 135060, 86158, 296264, 259285, 34487, 262611, 295401, 296279, 296303, 296319, 196679,
    296330, 295377, 296287, 296341, 86349, 75972, 296354, 296378, 228558, 230271, 231518, 172712, 198889, 260696, 296405, 109856,
    113079, 289810, 296414, 296423, 78858, 296450, 296469, 89014, 223434, 279487, 223408, 138104, 42589, 138147, 138167, 138116,
    233632, 46145, 160671, 18932, 232965, 18614, 46152, 179580, 160678, 281364, 281376, 296482, 281388, 140893, 296458, 295185,
    296502, 147013, 242662, 142349, 162372, 239295, 79705, 296522, 296529, 267277, 296538, 20129, 127386, 224205, 10608, 168841,
    168851, 136455, 238470, 162382, 296558, 116388, 282559, 148419, 148434, 271776, 19837, 174836, 268084, 148441, 110044, 109998,
    244606, 276989, 262975, 296576, 296596, 296610, 30185, 295467, 296636, 99616, 287862, 90362, 296692, 296712, 296726, 296737,
    296750, 296762, 296774, 203050, 296783, 180305, 295516, 250572, 292567, 278656, 295203, 296797, 294854, 296811, 294864, 296821,
    296831, 277937, 296851, 296878, 225842, 296890, 109593, 20230, 296901, 226089, 296923, 296956, 156919, 296931, 263476, 238517,
    282820, 295305, 296432, 147121, 138488, 257832, 296976, 296778, 111757, 113349, 227552, 136312, 259048, 296997, 224626, 224637,
    297007, 294929, 10640, 138591, 255567, 155028, 114654, 296943, 192057, 294940, 10652, 297017, 297025, 2492, 296803, 2502,
    208317, 242766, 297035, 279745, 110931, 262164, 167328, 167340, 262173, 20093, 297063, 135864, 295282, 136580, 295444, 228525,
    168815, 294488, 297084, 256567, 297096, 297104, 297134, 296512, 294209, 2913, 202405, 265652, 268571, 297151, 297160, 68955,
    297170, 297181, 69046, 297192, 282481, 282490, 216679, 297204, 216249, 226246, 296841, 297224, 12127, 268407, 226256, 216690,
    229600, 297243, 297215, 297266, 179760, 179773, 297288, 179784, 297308, 148311, 283028, 168863, 297324, 297333, 294637, 183391,
    203057, 295427, 296790, 297344, 135016, 297048, 167550, 196239, 297383, 135024, 297417, 191142, 224558, 194865, 194878, 194824,
    26895, 26926, 26906, 297444, 123517, 297459, 28663, 294707, 278447, 281711, 156611, 297475, 297488, 296442, 157801, 297501,
    297526, 297542, 292261, 118948, 295585, 296645, 287872, 297575, 112409, 126959, 200170, 256403, 297594, 116528, 94718, 115072,
    116483, 292067, 112463, 297626, 116629, 114323, 6158, 297659, 296586, 246717, 297675, 295043, 297692, 44119, 297704, 93675,
    297752, 133689, 297762, 297774, 283869, 232664, 297784, 243519, 297794, 297820, 297808, 297834, 297112, 297605, 297846, 297864,
    297885, 197915, 297897, 297907, 197923, 297918, 297926, 295057, 276376, 297855, 297935, 297955, 295025, 208241, 295064, 297973,
    297716, 297992, 297728, 298004, 298028, 298034, 79675, 296966, 298046, 298056, 297962, 297981, 298068, 298079, 298093, 45107,
    298112, 298132, 295263, 298152, 29917, 221742, 252539, 252547, 295607, 298179, 283038, 68962, 297355, 156931, 296864, 297369,
    298210, 42044, 102381, 298224, 195829, 297394, 298217, 228424, 294760, 298241, 116886, 45973, 298122, 298142, 298255, 298266,
    109167, 296013, 295651, 294897, 296657, 294909, 296670, 168460, 294729, 296683, 298276, 298286, 293009, 298296, 298345, 298310,
    270756, 298368, 298383, 11823, 295659, 234397, 263001, 298400, 298434, 298449, 152384, 294074, 157288, 261088, 296347, 298460,
    298472, 209289, 294620, 298492, 298502, 270710, 36376, 36386, 36397, 298514, 261539, 298466, 298482, 298529, 298535, 271717,
    92886, 298546, 298574, 298583, 298594, 298608, 298452, 274718, 141379, 208734, 298626, 298648, 298657, 141385, 281328, 244335,
    298668, 298681, 298703, 284684, 284691, 298688, 298739, 298748, 237978, 237441, 120098, 52032, 190234, 298759, 298774, 298766,
    120110, 298793, 267962, 298807, 211514, 298818, 75491, 298522, 207190, 298826, 298839, 75500, 65311, 22992, 175697, 292899,
    118613, 177457, 188318, 197616, 102171, 211212, 298860, 133245, 292760, 298885, 298912, 231608, 231616, 68974, 298921, 142536,
    298928, 298942, 276276, 298951, 298967, 86384, 298992, 298957, 298999, 298973, 299009, 299016, 229222, 4609, 174882, 276573,
    299025, 299033, 150701, 272176, 299052, 299057, 105304, 182200, 299065, 238368, 31886, 118972, 232899, 180261, 224145, 299080,
    224115, 299093, 224124, 299102, 38008, 105910, 203662, 299112, 299128, 38014, 203668, 38027, 105916, 203681, 38034, 203688,
    299147, 299157, 299168, 299180, 26450, 160686, 299190, 73204, 180978, 299042, 187905, 188008, 298713, 298696, 188014, 26620,
    48091, 164914, 210813, 128084, 26555, 103399, 299220, 299234, 283051, 106433, 127656, 50835, 85076, 117440, 55830, 299243,
    299252, 133151, 147878, 172650, 63599, 296601, 296697, 11767, 250081, 14551, 154131, 169186, 296618, 105311, 124384, 299263,
    299279, 23267, 5865, 160892, 299295, 205018, 299316, 190187, 253468, 293677, 293688, 267165, 92786, 161637, 102187, 296605,
    299334, 299344, 299354, 299365, 4288, 299373, 299195, 293269, 298854, 100265, 105358, 299382, 50145, 276616, 46828, 299393,
    299409, 299399, 75857, 299426, 293704, 299444, 52055, 67422, 231852, 299466, 299472, 253376, 102933, 196728, 30497, 295698,
    43652, 43591, 43599, 292904, 66104, 203621, 90212, 299480, 299491, 122193, 36953, 289258, 299501, 299518, 71965, 197646,
    68871, 77100, 191329, 278437, 195477, 298601, 299535, 298437, 183451, 299552, 243672, 234786, 61229, 140759, 286980, 183288,
    65980, 299185, 26456, 99460, 99467, 99475, 188573, 211840, 188941, 228799, 299568, 124091, 299591, 89856, 108157, 299575,
    211845, 99148, 65937, 299606, 119679, 274896, 299616, 237808, 299630, 65942, 299650, 299664, 299689, 299717, 299622, 274902,
    299735, 299744, 299752, 20874, 292549, 299768, 299789, 299638, 299777, 299798, 299824, 299810, 65949, 299657, 299611, 274911,
    65956, 89864, 211778, 254845, 254851, 299544, 125665, 147686, 298983, 100271, 88011, 126545, 299851, 65045, 299860, 84193,
    299867, 299882, 299876, 299894, 176177, 278452, 24302, 54234, 299909, 54239, 286275, 188103, 174509, 299914, 299926, 298554,
    298564, 100277, 237633, 238886, 238997, 299944, 299966, 299976, 299955, 299982, 300003, 300014, 100282, 106308, 300009, 300020,
    36960, 294597, 300041, 300051, 300064, 300078, 300056, 300069, 295797, 208888, 300092, 297198, 300083, 300115, 200516, 21377,
    300134, 64917, 183500, 185944, 100295, 100405, 100621, 298376, 300148, 298392, 298359, 133584, 122751, 133591, 291490, 300161,
    76768, 118830, 271177, 194655, 194665, 300181, 63603, 64884, 63619, 178012, 94111, 76627, 125753, 204235, 293383, 49061,
    299672, 299698, 86680, 100302, 100354, 100376, 76214, 133179, 160650, 262701, 57140, 35697, 300194, 134081, 196636, 299921,
    230312, 299387, 168677, 293709, 299451, 300211, 256142, 121285, 143651, 76637, 294456, 268016, 267985, 217016, 89656, 257100,
    300100, 300225, 300238, 300252, 92319, 34249, 84134, 111982, 300268, 169811, 300287, 300298, 203006, 87317, 300279, 187342,
    300311, 300331, 133390, 76973, 182881, 294608, 300188, 86482, 124131, 290831, 124344, 45910, 11228, 300347, 300358, 300124,
    292956, 292970, 86487, 182890, 145725, 188720, 300370, 116399, 98349, 187622, 76643, 187642, 299271, 86774, 300392, 189041,
    203741, 212749, 300401, 300411, 135464, 140136, 140151, 179357, 300419, 140168, 62207, 88574, 300438, 300443, 300449, 296296,
    127591, 299681, 299708, 299726, 299760, 298675, 199941, 66115, 74367, 259246, 127595, 154087, 196579, 196591, 125509, 105290,
    146843, 299435, 181992, 300462, 300469, 300480, 299203, 116060, 269286, 183201, 188650, 241564, 277609, 290985, 291050, 293417,
    300491, 300501, 78275, 300512, 121110, 300521, 204415, 292696, 300530, 300535, 300542, 300561, 300575, 300587, 15857, 77005,
    121078, 191407, 259717, 300598, 300608, 209197, 300592, 189013, 300620, 300228, 298403, 116406, 282571, 282619, 300635, 35647,
    300642, 300245, 300260, 237194, 237200, 300648, 300665, 300684, 300693, 300704, 156725, 103192, 149448, 300710, 284587, 300716,
    300725, 300735, 300745, 228442, 85374, 104052, 300755, 300774, 300784, 300795, 300803, 208391, 300812, 300828, 139872, 46800,
    77994, 114181, 202441, 62097, 300848, 300865, 228630, 138329, 212696, 300881, 300894, 300901, 230408, 285621, 300912, 300888,
    194777, 300926, 111989, 138187, 122141, 122153, 111994, 200155, 296257, 300939, 300950, 300959, 153648, 153663, 279524, 232215,
    232229, 300819, 300969, 153683, 33840, 300981, 300991, 301003, 301019, 301028, 301011, 14296, 301049, 84040, 261857, 64577,
    65197, 123891, 194389, 301067, 167176, 25001, 30383, 25091, 25099, 25109, 192743, 227068, 301082, 259671, 301087, 301092,
    301117, 301130, 301153, 301141, 240363, 244521, 52003, 52017, 301170, 301184, 284838, 110011, 110056, 301161, 301198, 102396,
    301226, 301236, 301207, 301246, 272073, 29109, 29142, 29176, 86398, 73332, 203308, 301257, 301177, 301277, 47062, 301291,
    129504, 301316, 301325, 48846, 286424, 301360, 301373, 301378, 263947, 263957, 73271, 49506, 282254, 205471, 301384, 301399,
    199001, 301409, 301432, 301420, 301443, 301455, 301467, 301484, 47787, 300974, 137472, 301493, 301500, 257425, 301515, 301533,
    301553, 301543, 34603, 301565, 301572, 237206, 241052, 212268, 237211, 301579, 301600, 301617, 301625, 194229, 301634, 74718,
    301649, 301654, 301660, 20138, 53706, 301671, 86404, 114188, 280085, 301693, 301711, 42801, 301722, 31905, 83756, 294288,
    301736, 209545, 209556, 39648, 55865, 209758, 45724, 301748, 301763, 301772, 100941, 301782, 301791, 301801, 123387, 301814,
    301839, 301856, 34551, 301753, 301822, 301831, 294498, 242372, 301847, 155476, 34560, 181756, 294779, 167185, 301872, 197130,
    301881, 63924, 78559, 182896, 301889, 78570, 301898, 301910, 177551, 214213, 301682, 205422, 213257, 301928, 213266, 301938,
    301948, 301957, 301967, 61281, 184269, 184273, 181581, 301975, 301984, 279534, 133253, 301994, 302011, 181587, 302004, 247411,
    240088, 45170, 302029, 302042, 301100, 301108, 228650, 183484, 302052, 302071, 302085, 302103, 302118, 302148, 302158, 182089,
    302170, 302182, 302133, 260325, 302195, 302216, 281638, 302200, 302225, 302208, 302234, 302250, 200404, 200413, 302256, 302261,
    302094, 302289, 99961, 94700, 286929, 302312, 232575, 58429, 302325, 217225, 54005, 137206, 141054, 35507, 270793, 282967,
    302338, 302359, 302367, 302376, 302384, 80250, 210546, 302394, 302429, 302437, 229232, 5362, 302347, 292351, 95531, 205620,
    111246, 291126, 98952, 302447, 302456, 151969, 239061, 302465, 302473, 65122, 144918, 146030, 146716, 34079, 217231, 217276,
    300720, 276916, 302483, 21334, 235934, 235953, 59154, 63041, 302498, 302504, 302512, 302521, 302531, 74957, 302542, 302551,
    302559, 224759, 289955, 295452, 295747, 98816, 136588, 98823, 136595, 181701, 61793, 181627, 282268, 302568, 61803, 181641,
    282275, 302575, 252444, 302583, 75277, 75285, 244783, 302600, 302614, 300836, 302623, 302634, 71974, 302645, 300841, 302604,
    302652, 301055, 28569, 302664, 61298, 302676, 302685, 240000, 302694, 302714, 216399, 234792, 302738, 139136, 170034, 283302,
    302757, 302772, 302779, 239185, 123714, 128625, 166839, 302792, 123668, 302808, 302819, 216408, 234801, 302747, 240009, 302786,
    97448, 248025, 216418, 302830, 236912, 302861, 146498, 266868, 266903, 302667, 170409, 302895, 170420, 274637, 302910, 261824,
    302924, 302933, 284511, 302943, 302961, 302972, 302950, 302983, 302994, 303006, 303017, 303026, 187482, 303052, 303036, 84557,
    303072, 86493, 303088, 166888, 166905, 77262, 249148, 96889, 77270, 303106, 147428, 147486, 147512, 182290, 294387, 112823,
    303128, 303160, 303179, 303210, 3874, 294402, 64508, 303232, 301475, 246315, 303248, 303260, 303274, 303288, 303266, 303299,
    35543, 269015, 278811, 161440, 151771, 144795, 151871, 161450, 116751, 284046, 80836, 139467, 232533, 303320, 303331, 303344,
    303353, 260630, 303367, 303386, 303418, 303393, 303431, 303463, 153118, 303484, 303513, 303522, 303538, 116867, 303544, 303564,
    303573, 97668, 303581, 72178, 72189, 303596, 52483, 52490, 303615, 38721, 67282, 67363, 4976, 303626, 303636, 70067,
    291090, 101837, 193793, 303643, 4944, 255828, 205271, 101850, 255839, 101871, 303655, 303668, 303696, 303676, 303705, 303728,
    303764, 303771, 303779, 303793, 228905, 15404, 15416, 75149, 194443, 130089, 143296, 102220, 17262, 303807, 303817, 183004,
    196460, 190801, 260744, 260750, 128633, 166847, 302800, 228983, 303827, 303858, 303843, 303870, 196394, 284598, 303881, 303887,
    131829, 214176, 303896, 21239, 124636, 303911, 213360, 11660, 264154, 303927, 303940, 303953, 195620, 303965, 153802, 139765,
    303972, 115598, 303240, 303988, 304005, 303996, 279919, 304013, 304022, 157187, 303933, 304046, 304027, 304068, 303904, 304053,
    304037, 304080, 304059, 304091, 61237, 3372, 304109, 166412, 180638, 180656, 304121, 182632, 242857, 242558, 243806, 276601,
    300217, 150965, 304143, 98216, 190939, 66860, 66869, 230674, 101275, 232473, 60230, 300319, 51892, 190750, 3523, 3491,
    122618, 197963, 304160, 304183, 304202, 304226, 66877, 304249, 123257, 304273, 52062, 208649, 304318, 52071, 304281, 63055,
    111097, 298935, 304330, 48333, 190700, 122121, 123175, 304343, 150974, 277979, 304192, 117788, 277989, 304362, 304369, 250552,
    199859, 304377, 57751, 117795, 303662, 106441, 99433, 79910, 304115, 304214, 304152, 304257, 196738, 261166, 304394, 135982,
    124351, 125469, 114046, 253520, 84092, 304408, 304420, 32257, 131835, 292815, 152700, 259156, 304432, 304456, 304471, 198023,
    87527, 256304, 304478, 198032, 256315, 87536, 256326, 2594, 285825, 170040, 240015, 4761, 302838, 212606, 2601, 166982,
    204567, 302845, 304488, 268604, 304500, 304516, 304533, 104809, 195687, 268619, 304508, 268629, 200428, 304547, 304563, 304577,
    304590, 91872, 283201, 304598, 304568, 53481, 124323, 195696, 304605, 304616, 241596, 304625, 302296, 153230, 304651, 213099,
    203371, 304685, 304725, 292400, 225373, 304737, 304757, 104818, 16626, 304525, 304777, 304796, 304814, 304840, 181497, 181506,
    304857, 304866, 304881, 304889, 72027, 269323, 73281, 198566, 304895, 304917, 103327, 288573, 288533, 304929, 304942, 823,
    34175, 244287, 304950, 275827, 304966, 166950, 166961, 296987, 194048, 103518, 304974, 194057, 304996, 305006, 305016, 305024,
    305040, 305050, 6945, 236416, 238562, 288956, 305060, 175608, 288809, 301367, 305085, 305105, 305118, 39865, 138713, 300551,
    32312, 19521, 219634, 226460, 303946, 305159, 38575, 305168, 162075, 166935, 305181, 108691, 305197, 105641, 305206, 247832,
    305218, 284608, 305226, 16219, 304402, 304657, 304732, 292409, 305235, 30601, 196767, 53449, 269913, 269930, 215428, 270610,
    305261, 305300, 305321, 305032, 253096, 253106, 305341, 305365, 305380, 305391, 305400, 305409, 305417, 305425, 305437, 104287,
    305451, 159122, 159147, 305469, 98149, 98521, 304491, 20383, 20335, 214449, 305478, 248777, 248813, 305486, 305501, 305509,
    305518, 301524, 194897, 241134, 108696, 305542, 305552, 305563, 57645, 22652, 305580, 43541, 305603, 305617, 206868, 305632,
    305647, 206892, 123775, 59411, 192925, 305662, 305673, 305684, 305693, 305530, 207013, 305704, 305712, 305729, 60430, 305746,
    305770, 305777, 305568, 305785, 305828, 305856, 305867, 305902, 60776, 132046, 279378, 153241, 305925, 305945, 14568, 14592,
    149988, 14539, 159298, 243599, 151332, 35905, 305964, 302878, 278419, 225594, 254950, 264097, 271640, 219038, 294795, 305984,
    18814, 306003, 306025, 251334, 305993, 306049, 306064, 306078, 18825, 306014, 306037, 140327, 306090, 91338, 305976, 306100,
    25731, 306108, 282538, 25792, 299836, 153459, 243812, 243821, 306123, 306132, 148194, 147086, 306141, 148693, 227719, 306158,
    71909, 306172, 276070, 91306, 246441, 251545, 306183, 306190, 207230, 207240, 223733, 306198, 306217, 94491, 188784, 306227,
    306237, 306247, 165666, 306268, 306258, 6168, 114211, 114219, 103336, 141271, 52406, 295889, 306303, 306311, 203911, 203920,
    302078, 25144, 306320, 306329, 272967, 306340, 306353, 306345, 306367, 304690, 81717, 282827, 206683, 306381, 114230, 265789,
    152367, 306399, 306411, 99293, 306421, 306442, 306453, 131272, 306466, 148981, 156138, 306390, 287054, 287065, 306490, 306500,
    294506, 270246, 270227, 306512, 267296, 267304, 306539, 297684, 288776, 306557, 211307, 306571, 43879, 306563, 306585, 306600,
    232585, 232618, 261208, 268178, 206735, 306649, 306671, 306655, 306677, 306692, 262342, 56435, 285478, 151337, 306704, 208035,
    306722, 269307, 105050, 306753, 110623, 303280, 164127, 306760, 306767, 107804, 107841, 153139, 306782, 30419, 304906, 207247,
    306798, 207253, 306207, 572, 611, 1490, 1392, 1379, 145128, 148204, 297428, 38048, 42468, 46544, 203702, 38059,
    42479, 46555, 203713, 306808, 306816, 306824, 306835, 306851, 102899, 102905, 306842, 306871, 306857, 191972, 263320, 37426,
    208042, 300858, 306888, 208049, 191976, 29680, 9321, 300934, 304544, 247842, 294435, 92697, 211727, 306905, 66631, 177823,
    281883, 177840, 232598, 232631, 232640, 285589, 285605, 236651, 306917, 306927, 306952, 132339, 131709, 306970, 306986, 306977,
    293080, 306995, 307020, 307040, 179952, 307003, 307059, 307078, 78201, 307086, 301038, 307093, 307109, 307118, 231928, 307128,
    307150, 307164, 52988, 307140, 301123, 287109, 298893, 307178, 301190, 166920, 307204, 307218, 301507, 38992, 173931, 173988,
    307245, 307255, 304440, 2020, 145793, 200311, 200318, 307267, 146477, 146509, 146559, 307287, 71656, 301608, 173936, 247187,
    92990, 270161, 307308, 307315, 218030, 305458, 83601, 307326, 307337, 211733, 24215, 112385, 256771, 22435, 127986, 246821,
    185700, 186510, 284147, 125034, 149490, 307349, 307361, 275940, 94325, 85895, 85903, 307384, 248886, 85911, 307392, 307399,
    307418, 307458, 307465, 96592, 275626, 92511, 307101, 77164, 181922, 307472, 272493, 79940, 146419, 307486, 307497, 307508,
    140003, 245073, 194492, 213340, 183880, 183890, 307524, 307278, 307554, 307563, 307536, 307573, 181935, 236475, 307582, 81022,
    84045, 246672, 53196, 307592, 307605, 307620, 307598, 307612, 307629, 252452, 239089, 295931, 85276, 87751, 158670, 283308,
    307639, 307647, 307068, 256910, 307656, 66589, 307689, 295938, 177151, 307545, 307711, 307720, 186516, 270168, 307728, 75427,
    85282, 134920, 191669, 303312, 294168, 307745, 307761, 140791, 307754, 307777, 307784, 307791, 307801, 95866, 307812, 101284,
    307826, 307831, 158528, 285060, 198008, 245079, 77902, 194499, 101289, 307846, 307853, 307863, 77910, 194285, 194507, 160372,
    305930, 307875, 65229, 86310, 307898, 307919, 307887, 307938, 141859, 307954, 307965, 307978, 81030, 38509, 122674, 130968,
    142127, 122679, 307992, 134695, 308009, 97561, 308017, 122758, 308029, 308040, 307184, 96895, 303377, 303403, 303425, 308068,
    303409, 303474, 153126, 303499, 303530, 303552, 303557, 303589, 303606, 307212, 295945, 281490, 128101, 15069, 146518, 244979,
    80620, 308082, 308087, 182348, 308101, 95113, 308123, 211197, 308137, 308147, 308156, 67619, 308095, 308165, 49337, 105063,
    308174, 308185, 75260, 131840, 224840, 200574, 308200, 308212, 308223, 308237, 308192, 308258, 67562, 308291, 182767, 47983,
    308302, 289473, 308311, 225716, 308317, 307480, 135239, 134717, 125159, 2125, 308325, 135248, 155307, 3650, 308346, 69993,
    308367, 218352, 260355, 260375, 218323, 308381, 308390, 281496, 303171, 56755, 308399, 52081, 231981, 308414, 142770, 308426,
    73736, 308452, 308433, 67046, 73745, 308461, 308471, 308482, 170240, 293586, 98739, 180856, 217998, 301217, 180877, 181178,
    194937, 228915, 293329, 98601, 308499, 308516, 39608, 80845, 260766, 308505, 308532, 308548, 277614, 287020, 291056, 164143,
    124184, 290496, 304695, 308579, 308609, 304705, 308589, 308619, 91715, 164164, 246678, 78121, 188227, 308600, 308630, 303683,
    303713, 303690, 303721, 92348, 130973, 308047, 96903, 308662, 255979, 304448, 308675, 78994, 103413, 103425, 59819, 59827,
    142446, 257278, 279861, 308695, 308706, 308720, 93299, 106056, 308737, 115795, 308741, 149550, 79284, 149559, 139378, 308749,
    308775, 308783, 242727, 146643, 261773, 16225, 301284, 75567, 173267, 219912, 75663, 77951, 74452, 37644, 237731, 92825,
    308790, 234158, 138023, 308815, 276946, 121967, 308825, 302319, 301061, 253734, 304337, 308840, 308851, 308846, 308857, 172888,
    67077, 169822, 308869, 308877, 193000, 211574, 308887, 308902, 308911, 308921, 308938, 308892, 193005, 308931, 217672, 250680,
    221753, 308954, 308962, 164180, 308969, 260131, 304988, 259111, 82665, 303441, 308987, 308997, 308698, 309007, 309015, 302401,
    309023, 302408, 306431, 254620, 80377, 302417, 309043, 4650, 193410, 16506, 196305, 276584, 309061, 309077, 309089, 309103,
    309121, 309133, 34629, 309144, 309160, 309171, 309183, 199369, 309195, 34639, 309152, 18201, 169547, 241062, 18228, 120799,
    309208, 309231, 309240, 301586, 301593, 94728, 156409, 309249, 94739, 156420, 309220, 309260, 158426, 173964, 256285, 309271,
    106658, 309284, 309301, 309319, 156111, 156150, 309345, 9343, 71601, 309370, 71638, 178176, 309383, 172218, 224728, 309400,
    309323, 93304, 309421, 309457, 309486, 83840, 6890, 169738, 222240, 309507, 309524, 46126, 309555, 93884, 127601, 296549,
    309579, 309595, 309468, 309476, 309497, 47364, 309618, 309631, 150940, 309643, 216621, 168797, 309661, 309669, 309677, 309685,
    309708, 242196, 309693, 309718, 180021, 230206, 309730, 309740, 309747, 309753, 309767, 185049, 136275, 83343, 155486, 309785,
    309797, 208059, 309808, 100748, 158303, 309815, 309822, 100758, 254605, 100764, 309801, 309828, 309839, 298947, 304862, 309856,
    309871, 45396, 74385, 304873, 180234, 242999, 309861, 308035, 208064, 309899, 208068, 306897, 309906, 525, 559, 1366,
    1279, 1245, 1257, 23003, 196251, 309943, 23011, 196259, 164068, 309948, 305243, 302719, 309955, 309964, 125681, 79777,
    309974, 151779, 144805, 151881, 309994, 310007, 310028, 310033, 310040, 243972, 293092, 293101, 308142, 310052, 310073, 9759,
    188242, 9767, 263577, 123296, 310096, 310109, 310101, 65204, 302917, 99441, 47233, 303801, 259171, 304238, 66885, 95620,
    310127, 74005, 133487, 210928, 310114, 84950, 26633, 59860, 164919, 161955, 302853, 310119, 182321, 297513, 263858, 310147,
    310161, 310168, 67092, 244965, 247195, 81936, 206641, 76775, 151342, 285001, 310182, 310202, 89869, 174646, 210999, 299508,
    205740, 16710, 238449, 255345, 310211, 307946, 218993, 238481, 310217, 153809, 292361, 48356, 258304, 102690, 45662, 81916,
    25374, 83819, 293644, 37769, 57694, 37777, 203120, 86013, 309067, 310227, 117263, 182594, 191074, 303449, 310243, 310255,
    310269, 310286, 310261, 182808, 310299, 310321, 64385, 310293, 78514, 78532, 2029, 78582, 310349, 310363, 310375, 310154,
    310382, 310398, 310389, 106900, 234143, 95137, 247853, 76724, 304663, 310429, 310448, 250111, 310434, 310460, 250118, 310467,
    12914, 302423, 309033, 125299, 310480, 151488, 225935, 240700, 310498, 264173, 263758, 310521, 310528, 35250, 67763, 310441,
    310453, 310541, 310568, 310581, 310594, 310626, 310655, 63667, 310662, 310057, 310674, 303455, 310702, 98610, 283218, 54460,
    269406, 194372, 310713, 99513, 310062, 310736, 244649, 310081, 99519, 182374, 310549, 55299, 279888, 310606, 310556, 310576,
    196197, 310750, 242780, 261500, 310617, 139706, 310757, 88483, 88498, 310763, 310769, 88504, 290891, 310088, 294179, 310723,
    80680, 310779, 310795, 310811, 310788, 203129, 16658, 307906, 127111, 137875, 310708, 310742, 310820, 275634, 310831, 310847,
    281443, 153299, 153357, 61384, 184649, 310865, 283979, 283995, 310880, 255370, 281566, 310896, 310936, 196950, 219917, 219931,
    37786, 37793, 86182, 153474, 46814, 65868, 203138, 272375, 281571, 310901, 310956, 310968, 173655, 310974, 310987, 65875,
    281448, 311000, 311009, 188087, 68215, 137774, 311019, 284019, 311033, 133701, 136145, 284029, 63171, 77718, 210046, 196203,
    310068, 310679, 76372, 310870, 311055, 311060, 51547, 310888, 223205, 311024, 311069, 181317, 278155, 182688, 278162, 310488,
    311084, 311124, 311095, 311104, 311135, 118861, 310994, 64018, 310962, 11518, 310308, 64396, 289302, 310336, 122292, 150006,
    301562, 277223, 302724, 311145, 242457, 242418, 242439, 188400, 78772, 96273, 217237, 17071, 96240, 62142, 53332, 202847,
    64023, 64779, 129826, 132111, 35549, 295815, 177686, 311158, 311166, 228921, 228928, 228936, 228945, 120458, 52615, 56988,
    64029, 253064, 185415, 161698, 311175, 155463, 151348, 188472, 311199, 311216, 311239, 311252, 311272, 285066, 311281, 301863,
    94400, 61883, 94409, 38133, 259056, 311292, 311309, 311348, 311366, 311227, 64036, 64785, 67688, 116067, 124725, 64792,
    144406, 125707, 230365, 311401, 60333, 267250, 296701, 166864, 304786, 41981, 212952, 270254, 295677, 25643, 242594, 307668,
    306523, 191543, 301918, 292653, 306533, 122021, 58953, 164202, 193571, 302243, 164230, 235083, 235091, 2325, 127116, 2360,
    82902, 123577, 189056, 290233, 82908, 189104, 311408, 189062, 311426, 303980, 311455, 263642, 311464, 311483, 311501, 146094,
    102829, 102853, 311527, 311532, 311540, 311549, 82916, 82923, 281655, 311492, 55622, 288278, 311564, 101450, 311573, 311586,
    101459, 49937, 78265, 311595, 311601, 267236, 52500, 288283, 58491, 72199, 307985, 288289, 58498, 52506, 262009, 300142,
    64103, 311609, 311623, 178495, 232556, 311642, 311614, 180800, 311651, 311661, 311672, 63340, 311509, 311519, 120209, 126589,
    126594, 126746, 263076, 263092, 17509, 103852, 196400, 21385, 311681, 311690, 237774, 78779, 302023, 301077, 311709, 161060,
    38514, 152477, 305092, 306474, 198061, 154246, 198071, 311725, 311737, 311750, 311766, 311788, 311810, 311820, 311777, 311799,
    311757, 311832, 311846, 290633, 50448, 70100, 75829, 198978, 285739, 174995, 311862, 260647, 75833, 75842, 120311, 306375,
    274790, 128115, 259094, 137480, 123185, 311875, 305738, 64861, 158313, 192475, 132821, 311882, 65883, 151898, 223214, 311888,
    311895, 54281, 144377, 311903, 311918, 311933, 311955, 311981, 311968, 249222, 249234, 169765, 243460, 206847, 156119, 305098,
    305129, 305139, 29797, 29770, 84113, 104563, 312007, 312016, 312026, 19792, 207534, 300656, 312067, 312083, 148228, 312094,
    297299, 297317, 148321, 24221, 203105, 312106, 312113, 191588, 65593, 312122, 312136, 312148, 312162, 152483, 251733, 198080,
    152490, 251740, 198091, 222188, 222197, 166645, 253079, 310508, 264185, 213634, 213646, 311114, 70048, 243337, 312179, 37375,
    312193, 83806, 245394, 34922, 312202, 312211, 312220, 312231, 126567, 156954, 41945, 283059, 283068, 68433, 68440, 312247,
    269450, 124730, 38932, 185027, 231340, 312265, 312274, 88081, 312289, 312299, 11781, 259609, 312279, 312311, 213655, 261365,
    213668, 213730, 261378, 311869, 312331, 312269, 268526, 4212, 246161, 94440, 183631, 304670, 308130, 50423, 312346, 13996,
    211948, 306059, 312363, 312370, 309848, 272951, 300108, 274307, 199144, 56673, 138333, 143734, 252522, 296313, 199785, 154093,
    83350, 57911, 312256, 312376, 259726, 222205, 290452, 175860, 65676, 243762, 292448, 179801, 312389, 208826, 123312, 153485,
    42686, 54247, 224243, 297144, 312354, 239669, 7890, 258096, 281233, 200065, 312415, 312429, 17516, 152034, 312445, 312460,
    112303, 312478, 312499, 312523, 312509, 312533, 312547, 312560, 312568, 312578, 312588, 312597, 312610, 239333, 239348, 312489,
    312620, 312644, 312630, 312654, 312668, 233638, 312681, 312690, 43726, 257025, 312700, 225558, 305202, 305213, 311938, 312718,
    312735, 311942, 312722, 311947, 312727, 312739, 223928, 291901, 306151, 39476, 312747, 312766, 312779, 312795, 299900, 312825,
    84203, 312773, 25036, 308443, 167161, 312839, 73002, 312851, 312860, 48366, 141278, 52415, 52424, 237525, 310235, 17722,
    312869, 238830, 312884, 157194, 33279, 312923, 194457, 312929, 174956, 158378, 15430, 75163, 154975, 312945, 312894, 312905,
    37221, 117292, 180124, 28890, 117302, 189728, 312787, 312810, 57005, 283121, 130040, 312845, 312957, 244067, 312977, 179914,
    236690, 313022, 313032, 313043, 313067, 313079, 148637, 185890, 313055, 313090, 148648, 276030, 148660, 185901, 193497, 313102,
    252421, 40569, 208834, 282023, 151681, 57023, 131469, 313120, 151689, 215664, 172823, 57053, 172829, 152863, 272081, 243721,
    30655, 30713, 30663, 313136, 313148, 313161, 313175, 255398, 225307, 267379, 299322, 284312, 270722, 154693, 270734, 22607,
    175628, 204420, 212587, 90457, 313191, 294087, 286374, 286388, 213708, 175375, 286401, 171010, 144865, 303140, 172343, 303148,
    170105, 170121, 313210, 313242, 313261, 279974, 313216, 27530, 313229, 313274, 196430, 100839, 313252, 313285, 239916, 190478,
    313300, 313314, 62882, 165979, 161615, 194547, 194555, 194676, 194690, 160023, 204923, 215674, 313328, 313344, 69885, 117955,
    313350, 190243, 313358, 215679, 313333, 313368, 313376, 190194, 211443, 102358, 32526, 32537, 132314, 32546, 313385, 313393,
    131388, 175907, 313406, 313339, 84544, 161229, 313417, 288200, 261393, 277148, 77186, 281671, 146424, 307491, 310941, 313431,
    306730, 307516, 230481, 313437, 59946, 201066, 33010, 313423, 313456, 313468, 236956, 313482, 4998, 45745, 45760, 288209,
    200797, 228474, 313503, 73472, 313523, 313533, 313542, 5055, 313559, 313550, 313567, 313573, 106521, 115804, 313588, 313581,
    313602, 313611, 205542, 240812, 69012, 163792, 190125, 25850, 313621, 271575, 313631, 251476, 29369, 313641, 67931, 30722,
    296492, 313662, 271585, 251344, 313682, 313695, 271598, 313651, 313710, 313722, 17559, 185428, 200684, 301264, 310047, 313733,
    60278, 139784, 60284, 313745, 170946, 301269, 128270, 313757, 313769, 130002, 283129, 310907, 313789, 131947, 313812, 130050,
    131959, 224060, 180883, 130009, 283136, 310947, 130059, 300155, 181274, 313834, 313844, 240746, 169194, 194806, 194836, 194845,
    73132, 73176, 73141, 73185, 73509, 240760, 240770, 313856, 313863, 231839, 310248, 310838, 307431, 307444, 249807, 310855,
    102192, 145504, 296706, 299286, 149523, 296626, 64050, 283145, 310914, 313796, 64058, 64083, 139825, 133566, 278990, 313872,
    105680, 139933, 234180, 62946, 313880, 293257, 300474, 227567, 239122, 275407, 313887, 313910, 313928, 239129, 313897, 110769,
    313921, 313939, 276825, 287833, 313962, 313990, 313974, 123589, 182330, 65287, 311246, 314012, 313492, 246912, 314018, 230331,
    314027, 314037, 89877, 260072, 314048, 314076, 57319, 314094, 108168, 187761, 314054, 314103, 8819, 314064, 181184, 314117,
    215685, 276489, 145752, 243828, 276497, 304169, 72914, 276089, 200690, 45263, 231354, 282640, 73427, 116586, 314124, 314141,
    293750, 314131, 40644, 314157, 40650, 314163, 73767, 71095, 314172, 240819, 68983, 186156, 193831, 238854, 314198, 314208,
    217753, 193838, 107648, 69056, 147520, 303190, 313803, 231543, 303199, 303221, 173464, 105893, 131395, 15103, 259759, 75114,
    205512, 234358, 176711, 190134, 5004, 189484, 51040, 219443, 313111, 231527, 185434, 200696, 231363, 101296, 280631, 194107,
    172872, 152059, 314219, 314225, 183638, 304677, 314233, 314243, 314251, 288837, 125905, 65889, 290906, 313946, 99325, 125477,
    142132, 314269, 314286, 210080, 154849, 99331, 310192, 55632, 79541, 274947, 304290, 304300, 57245, 200005, 304308, 130495,
    110780, 110807, 125912, 310921, 314307, 292332, 314315, 80404, 303786, 124191, 314325, 37653, 314335, 37660, 314345, 314380,
    281850, 158957, 231372, 310929, 313954, 314406, 314419, 306360, 142697, 230376, 234948, 128772, 314432, 216224, 314450, 314470,
    314492, 314514, 114851, 314538, 314460, 314481, 314503, 314526, 314559, 314573, 199428, 199438, 314548, 192726, 145523, 205487,
    311210, 68446, 34532, 204250, 314598, 314604, 35770, 269498, 314615, 85959, 149032, 182974, 295974, 39249, 208139, 314628,
    302706, 314652, 314660, 258076, 314622, 305112, 305149, 135287, 145313, 314669, 314687, 314739, 314750, 314769, 314760, 216461,
    286086, 106786, 132690, 155358, 314782, 150881, 92354, 204673, 314813, 277500, 290239, 306712, 67663, 86913, 113783, 314824,
    164578, 314566, 203841, 244366, 314836, 314851, 314873, 114903, 114912, 314896, 27508, 314914, 314934, 48193, 277363, 292825,
    314830, 48604, 295090, 169555, 314967, 314993, 169568, 314980, 315006, 315019, 269226, 19848, 189255, 315038, 315046, 315055,
    315070, 315076, 315090, 50538, 315105, 205238, 238749, 288508, 23371, 23353, 75473, 39522, 14501, 42450, 50082, 64734,
    315122, 3998, 49355, 315130, 39145, 238754, 312757, 30987, 147394, 164757, 167718, 315136, 315144, 315153, 121093, 290179,
    143941, 134145, 209821, 243176, 49839, 121388, 315173, 312073, 315192, 315206, 143952, 159642, 218647, 314580, 286097, 8979,
    232710, 315181, 178640, 206304, 105736, 315218, 315247, 315273, 315286, 236361, 300485, 119424, 315226, 315301, 315259, 179820,
    98565, 315356, 189115, 189069, 315370, 26463, 315376, 315384, 205058, 315402, 232260, 315392, 35655, 259137, 315411, 311700,
    61477, 315418, 315429, 315438, 315450, 170651, 313594, 3767, 76555, 93836, 144362, 134362, 206659, 206667, 315468, 315478,
    315459, 315490, 315496, 39345, 238763, 292992, 69626, 307332, 314819, 5191, 268679, 89423, 314181, 20742, 315502, 315515,
    315530, 315539, 67447, 184733, 314745, 315548, 210770, 314776, 281292, 314409, 315559, 117748, 315588, 315608, 315595, 315615,
    315627, 315639, 315651, 37389, 223841, 315679, 315724, 43303, 315735, 43314, 198630, 315746, 72073, 315767, 315793, 315780,
    315805, 39162, 176547, 149891, 128383, 211377, 66000, 312175, 130322, 288215, 232974, 288241, 44661, 195117, 259144, 44754,
    203722, 38105, 234850, 259064, 311319, 311358, 315818, 24077, 315855, 315865, 139047, 139103, 139115, 3214, 212708, 243618,
    281677, 243624, 36741, 169718, 315876, 35670, 170304, 315886, 60367, 128709, 250560, 315898, 209181, 76781, 239644, 177745,
    122765, 177693, 76789, 174806, 122774, 247276, 157202, 110020, 110065, 313128, 151698, 15174, 35596, 13519, 35681, 303064,
    300202, 315062, 306828, 306862, 306878, 315162, 203176, 307372, 211053, 315916, 315926, 205823, 315938, 183779, 315960, 315974,
    315986, 315993, 315999, 316005, 316013, 179647, 179657, 179597, 216587, 316022, 316032, 269293, 285805, 2195, 71720, 191240,
    191250, 316043, 285811, 316052, 316062, 314942, 162576, 191831, 237125, 299485, 128683, 242326, 283595, 279818, 249436, 237141,
    211221, 68877, 182501, 197655, 203578, 183334, 195649, 65024, 82752, 82932, 82940, 69606, 315980, 316074, 124985, 124993,
    316086, 63411, 316104, 178925, 316112, 138514, 252007, 138522, 252015, 308217, 308247, 138528, 308269, 316120, 182742, 316138,
    90392, 316147, 316155, 182749, 173177, 261443, 316080, 203850, 285897, 314845, 314860, 10108, 314886, 53397, 316130, 316174,
    316180, 316193, 316204, 138033, 314148, 316217, 293760, 293771, 216024, 316235, 3292, 152286, 309535, 316253, 288335, 144322,
    178988, 291211, 308862, 316281, 15735, 115052, 235646, 34410, 316268, 288349, 316294, 92397, 178103, 249039, 185099, 316306,
    316322, 127634, 316338, 316364, 259071, 316381, 311327, 98247, 98322, 316390, 182600, 74725, 53958, 316411, 316422, 316433,
    316455, 316465, 316444, 53599, 196930, 316477, 316490, 92360, 316503, 316515, 118510, 178622, 303043, 79835, 231868, 166478,
    293714, 316227, 48874, 316527, 316536, 286540, 316544, 48620, 278029, 129260, 299210, 12920, 316563, 182817, 296365, 296392,
    40339, 34420, 34442, 316594, 316628, 316661, 316600, 316634, 316614, 316647, 5306, 148881, 316686, 171728, 316703, 316711,
    316739, 84836, 316726, 316751, 316509, 48037, 316760, 48043, 316766, 13968, 311183, 316776, 168931, 265301, 299086, 299121,
    315756, 316792, 184738, 308559, 308569, 62423, 115812, 294470, 308539, 38948, 169596, 209104, 316785, 61303, 209134, 152875,
    272093, 66221, 92404, 152962, 316814, 316828, 316803, 299137, 178183, 300025, 316841, 16683, 48066, 48881, 316854, 299527,
    10787, 156882, 314189, 156891, 156902, 316862, 310355, 316869, 112120, 112129, 316876, 302491, 21344, 86989, 78710, 134474,
    175653, 314422, 66375, 311192, 192704, 258195, 115131, 226621, 101304, 11523, 286936, 267495, 314296, 92366, 57198, 57207,
    268348, 296215, 76534, 259211, 299360, 302905, 316888, 67290, 157957, 316893, 67244, 310001, 249863, 308420, 316903, 315949,
    153190, 16538, 315967, 76847, 316923, 103147, 274185, 64667, 316936, 21393, 316068, 99339, 301809, 26471, 20920, 132431,
    233483, 53340, 35917, 304265, 290995, 316953, 150566, 132439, 154765, 39260, 108084, 132449, 150572, 316964, 132415, 203856,
    224172, 314924, 281545, 7549, 131914, 175435, 316985, 316998, 156083, 202094, 116795, 175457, 96250, 202101, 116804, 316991,
    39386, 39405, 207037, 317018, 317030, 100080, 117129, 61834, 133358, 61894, 61904, 66553, 281463, 19889, 244297, 305592,
    317044, 317055, 213922, 317097, 317120, 212731, 143585, 251506, 272342, 215724, 317141, 317156, 317130, 317169, 177204, 159800,
    161580, 298233, 317189, 317180, 100693, 317205, 100702, 317214, 66557, 281467, 317236, 231068, 153853, 317255, 317066, 317273,
    317304, 317264, 317076, 317283, 317316, 310406, 317328, 317337, 317348, 317358, 317369, 317402, 317416, 72845, 116368, 162864,
    223821, 317109, 286118, 286130, 268188, 217549, 310417, 317430, 306774, 43830, 317381, 317449, 317471, 317460, 317482, 317493,
    317512, 65661, 135170, 312036, 312051, 107229, 107244, 167360, 250966, 109608, 109666, 317530, 317540, 184412, 224367, 317551,
    317578, 317564, 317591, 160269, 225763, 263615, 305880, 305890, 305914, 317614, 317630, 317643, 317621, 226470, 298324, 317656,
    298335, 317694, 317707, 253652, 317720, 7566, 189339, 7576, 306277, 114237, 306288, 206396, 296913, 109962, 317737, 96392,
    317746, 317756, 317764, 232374, 232385, 232343, 4140, 99279, 243403, 296565, 267706, 267733, 85185, 228453, 300766, 261193,
    312383, 317773, 317784, 317793, 3729, 279593, 141446, 317802, 308669, 237783, 237796, 279787, 317812, 52690, 5386, 249279,
    9807, 164106, 38112, 38141, 311377, 311389, 317830, 317439, 85385, 201216, 317844, 266280, 289828, 208857, 217573, 221767,
    303736, 317862, 139546, 317887, 317874, 299888, 317896, 317817, 317903, 90271, 317925, 317911, 317950, 190588, 317965, 317980,
    317997, 190632, 317931, 318014, 318029, 318052, 318088, 318064, 7051, 315827, 318107, 102731, 258105, 194745, 211932, 311078,
    151979, 244256, 258115, 103806, 135293, 317943, 317988, 318005, 318021, 318040, 318075, 318097, 7061, 315837, 318117, 306116,
    315848, 14904, 308682, 308688, 109971, 154054, 318128, 318140, 314082, 191560, 72055, 123810, 260026, 91990, 238291, 136790,
    238303, 318150, 11669, 318172, 318187, 44822, 318208, 318227, 318237, 275600, 38877, 91177, 205201, 206494, 306959, 300426,
    295146, 103895, 318154, 318246, 318263, 95463, 318269, 95581, 311990, 318180, 311999, 206475, 318217, 44831, 275379, 95473,
    318163, 318277, 246050, 44165, 318253, 74242, 179372, 318300, 6907, 318312, 113959, 272138, 271829, 272198, 268301, 94290,
    146101, 311470, 111888, 74522, 169318, 127851, 318333, 307230, 272664, 160466, 169333, 271650, 197831, 171563, 318353, 238532,
    272572, 272773, 318318, 293986, 318375, 318398, 318419, 198639, 318435, 318446, 7869, 136978, 318481, 318496, 318510, 245914,
    90487, 268540, 318527, 318560, 318576, 318543, 318594, 169345, 318384, 95006, 94898, 214473, 318365, 318407, 18836, 18852,
    18867, 318618, 318642, 318664, 318676, 297668, 318689, 241256, 242204, 5763, 13911, 318708, 43841, 318735, 43854, 318748,
    318719, 242233, 283904, 283934, 279755, 318762, 279765, 287034, 318772, 318783, 290161, 318796, 52701, 162091, 287982, 18631,
    113532, 113541, 113555, 113568, 311337, 318812, 318823, 177558, 318606, 318834, 318846, 312710, 317197, 318859, 81165, 272301,
    318868, 91887, 222099, 318892, 112339, 184754, 200122, 211322, 74685, 318905, 12949, 104857, 318920, 318910, 318938, 195155,
    318950, 318957, 318966, 59209, 303254, 311714, 318976, 302592, 270563, 270187, 59247, 318987, 318995, 58966, 319004, 287993,
    319015, 302763, 102571, 272350, 195665, 169153, 319038, 169282, 169359, 169290, 272621, 319062, 153037, 280020, 280028, 319092,
    101368, 189078, 20242, 138866, 186052, 164925, 164955, 311300, 161933, 100531, 164964, 242573, 317503, 312467, 312472, 159088,
    312453, 134884, 179958, 189971, 319112, 319124, 231693, 319135, 179966, 239212, 319145, 318804, 246737, 18292, 259686, 259735,
    290197, 319160, 319167, 319174, 310561, 319194, 303743, 130022, 257978, 319209, 319219, 85967, 153652, 199472, 286942, 319234,
    146114, 319258, 319242, 319250, 53157, 78063, 318304, 90251, 136711, 319283, 189940, 319289, 65897, 293720, 319299, 96262,
    319319, 319325, 319310, 198982, 286547, 155375, 131688, 155381, 283152, 131584, 319269, 55791, 319227, 319332, 183682, 299496,
    319348, 144047, 144056, 300455, 279599, 232511, 279615, 319359, 319366, 300629, 319202, 319183, 319376, 319395, 319406, 72747,
    72758, 317852, 319418, 319428, 53348, 319439, 319450, 69402, 283520, 26333, 53202, 319467, 319478, 158759, 76161, 319499,
    319510, 46106, 310684, 182006, 175634, 310875, 281577, 182513, 51254, 283784, 260804, 319518, 319527, 97456, 281509, 241192,
    319353, 241206, 319536, 241215, 67504, 209453, 117231, 157692, 319572, 101894, 319592, 276469, 319605, 303752, 282652, 130027,
    257983, 319214, 111165, 240913, 286519, 61483, 61519, 177054, 206585, 283643, 155668, 319622, 165449, 235970, 283272, 123067,
    67856, 67897, 319642, 319654, 319664, 319674, 309052, 319684, 319695, 319705, 319716, 319724, 309083, 309096, 309112, 319734,
    319742, 160582, 319750, 160591, 319759, 319770, 319781, 319791, 319802, 204070, 319812, 119057, 119073, 230190, 289788, 30786,
    56263, 204018, 108905, 319824, 108917, 319836, 11080, 190284, 319848, 319858, 319870, 141160, 39748, 319894, 319919, 305493,
    319947, 319958, 319981, 320007, 319994, 320020, 320033, 319969, 320048, 320061, 320071, 320083, 320096, 24052, 320111, 320123,
    320137, 226866, 320148, 319907, 320160, 320171, 320181, 320190, 320201, 320211, 197243, 320223, 320258, 320235, 320246, 320270,
    90659, 305760, 320282, 320292, 309760, 309776, 274581, 309791, 104063, 210570, 320304, 270928, 320319, 270811, 270824, 320352,
    270836, 270849, 270860, 308757, 308766, 320385, 244420, 320397, 73259, 292291, 319929, 320415, 320420, 252584, 283322, 320426,
    320437, 244424, 184907, 320451, 320407, 320462, 320475, 320468, 320483, 184913, 320457, 120499, 189427, 201277, 320492, 320508,
    320497, 320520, 320531, 297122, 30907, 51098, 246685, 300921, 51110, 315115, 95144, 106317, 51063, 204151, 299073, 198483,
    320548, 320553, 203285, 320565, 223045, 320581, 203293, 320573, 306923, 99639, 320600, 285159, 320607, 320645, 320615, 320659,
    178941, 94589, 235520, 293335, 99925, 235541, 308728, 320670, 320689, 179081, 27854, 320680, 29211, 89335, 302305, 57081,
    320698, 67960, 103812, 119209, 320704, 320711, 320718, 97107, 185794, 97239, 320628, 320653, 320634, 267400, 320730, 227534,
    320739, 177611, 320747, 320752, 60907, 60978, 193427, 320759, 60918, 130519, 104172, 23144, 58639, 94818, 170872, 190994,
    312985, 94828, 197474, 218181, 208468, 320781, 320788, 320796, 320803, 206319, 101973, 26055, 304324, 278262, 320811, 320821,
    320838, 320844, 320851, 320859, 320868, 320876, 239865, 320884, 320893, 320903, 181827, 320924, 251492, 320952, 320829, 320957,
    150629, 231174, 320936, 320963, 154911, 320815, 320982, 178273, 320987, 320974, 320994, 321010, 273075, 61460, 282226, 321020,
    115259, 143076, 274208, 274242, 321033, 274220, 321052, 321060, 282231, 146797, 164236, 235069, 79102, 239677, 117374, 321069,
    269391, 321074, 74252, 246924, 321082, 321104, 321128, 321093, 321116, 321140, 320388, 166033, 321161, 321169, 321177, 321181,
    212966, 313413, 321188, 321201, 321192, 321212, 321226, 321217, 83247, 321238, 261472, 155688, 261480, 35834, 321249, 92003,
    286431, 321231, 321258, 321206, 321264, 321279, 48853, 321271, 195703, 321297, 212920, 212933, 47006, 177985, 54967, 95203,
    288177, 245604, 245644, 321324, 245611, 321331, 135299, 105578, 105604, 130725, 321342, 321347, 321357, 321365, 167592, 241623,
    282497, 51963, 239683, 282922, 80928, 152793, 321376, 201304, 321389, 7154, 7183, 321404, 321412, 200526, 321421, 321438,
    321446, 321430, 321456, 321469, 321478, 321462, 258754, 181656, 301642, 95154, 321383, 321397, 259764, 321488, 79353, 293339,
    53775, 293357, 321499, 307409, 321514, 60929, 60937, 60990, 294442, 321494, 138938, 189153, 76324, 86412, 321523, 61145,
    283752, 283763, 308208, 71276, 95120, 265038, 79191, 79208, 79196, 316911, 79397, 79408, 77520, 79492, 169613, 320364,
    320330, 320340, 320374, 321542, 321554, 320724, 90166, 321564, 321580, 321605, 321617, 321628, 321567, 321643, 321657, 92204,
    92122, 92171, 92132, 321695, 321708, 69896, 95332, 61010, 149827, 226144, 321720, 321727, 321737, 321762, 321767, 128822,
    321776, 70772, 150479, 236021, 178511, 321746, 26485, 85613, 117837, 243849, 321793, 321811, 26491, 321832, 321754, 256236,
    321818, 321859, 321868, 247378, 321712, 321825, 321880, 321583, 140314, 321894, 321646, 171076, 251025, 289057, 321907, 321917,
    163374, 122627, 34029, 321931, 321940, 95873, 68220, 321949, 321977, 321989, 286553, 4682, 10797, 10822, 10827, 10835,
    10802, 60520, 60527, 60535, 322009, 305268, 305307, 305275, 305287, 305328, 137917, 154229, 322027, 322038, 322050, 322062,
    285517, 320665, 128549, 322081, 322089, 321901, 40459, 322096, 322117, 322129, 322106, 322142, 322150, 280668, 81420, 322160,
    322170, 322185, 303917, 308335, 322197, 322218, 322212, 322238, 322253, 87720, 261455, 105939, 218112, 298162, 60948, 298170,
    156311, 322274, 322284, 321698, 322229, 322245, 322263, 290127, 297584, 131607, 41860, 322313, 58976, 322328, 147652, 166557,
    257849, 256507, 256423, 318286, 203468, 25549, 316162, 319940, 305068, 217135, 322318, 322345, 322356, 7304, 19500, 321152,
    19459, 136281, 10, 24, 294819, 294830, 106256, 150074, 243529, 150155, 150169, 296719, 268589, 322367, 219882, 322391,
    322379, 322406, 322428, 322417, 322439, 322450, 32103, 309390, 309376, 322470, 322485, 322496, 322507, 137573, 178948, 14,
    322517, 322538, 254410, 40068, 236330, 322478, 316288, 154455, 151840, 288650, 279, 176602, 294150, 310638, 182381, 322016,
    202305, 244665, 244684, 244693, 244716, 321572, 321592, 321608, 257783, 214131, 236586, 321620, 322551, 322558, 307928, 265456,
    322575, 166102, 322590, 308522, 111564, 138545, 162394, 284969, 110360, 162976, 166150, 138554, 162988, 322595, 322604, 131180,
    55926, 276750, 213206, 259524, 322293, 259540, 283006, 317224, 322301, 2990, 144195, 187499, 322634, 44554, 207480, 174759,
    322650, 187461, 322659, 149082, 311908, 71411, 304129, 322670, 322676, 127876, 952, 130149, 322687, 165704, 322701, 322709,
    322718, 169000, 169043, 47562, 92966, 138441, 47571, 95796, 95808, 122633, 122639, 238897, 29866, 29848, 236450, 322731,
    322750, 278475, 7799, 186264, 291932, 16810, 322770, 322795, 79603, 322833, 322844, 263977, 63786, 192895, 322857, 322870,
    87648, 103664, 197159, 322879, 232111, 308732, 308711, 98052, 322890, 57365, 197896, 322902, 322939, 61016, 246170, 246182,
    322971, 322976, 322791, 322992, 178451, 211585, 283469, 178459, 211593, 283481, 314638, 300729, 167663, 322997, 323003, 323010,
    323019, 323030, 274157, 323037, 159933, 323044, 323063, 323074, 323086, 323097, 159941, 323109, 323119, 214349, 214362, 214372,
    214381, 20147, 127393, 323131, 323143, 205402, 290010, 322582, 81752, 81760, 323157, 323171, 323187, 323202, 323219, 323248,
    323282, 323265, 323300, 323231, 323318, 323345, 323355, 323334, 8908, 323369, 301334, 301347, 323379, 24901, 24909, 309879,
    81608, 323388, 134494, 234999, 323398, 323409, 63499, 205782, 205830, 315364, 323428, 323437, 237316, 176967, 323446, 61102,
    301303, 323454, 252568, 182389, 310589, 312952, 254922, 226755, 323465, 143380, 179217, 238840, 310646, 312914, 280333, 278306,
    251842, 261802, 961, 323482, 212470, 243572, 212477, 40306, 380, 418, 857, 833, 142332, 267993, 267999, 237587,
    277292, 96127, 98018, 323495, 322882, 288223, 7163, 73482, 313513, 170173, 277457, 291192, 170182, 16044, 73, 41202,
    114584, 323515, 220403, 323547, 78, 97, 222, 323580, 250587, 298800, 316821, 323598, 47992, 65623, 178727, 154533,
    282037, 178738, 300033, 304136, 47373, 244263, 215038, 323610, 154289, 323621, 309915, 1263, 117269, 180751, 180764, 323589,
    73347, 151024, 151249, 275897, 157241, 323645, 86094, 308055, 159523, 323679, 323685, 323693, 67430, 280165, 290928, 308062,
    283363, 323700, 323714, 82758, 230809, 250246, 5267, 5288, 316402, 98260, 98332, 150856, 171576, 307502, 319599, 144964,
    190206, 40019, 40039, 91599, 323729, 323737, 171587, 196352, 323747, 179256, 209121, 272862, 285135, 291362, 48417, 302730,
    66402, 127017, 132796, 154775, 132807, 90851, 186712, 311151, 134655, 268416, 188195, 323758, 260429, 137998, 323792, 323804,
    80755, 323817, 323825, 323835, 323848, 322814, 322823, 323864, 323871, 323879, 323891, 323905, 323917, 321601, 322864, 309886,
    307840, 321888, 323934, 46978, 322694, 297892, 205529, 323947, 323960, 136218, 118104, 118113, 323990, 96599, 286478, 318982,
    261996, 196268, 233705, 290874, 168572, 175534, 235853, 324004, 323925, 303081, 323488, 322567, 135845, 321636, 103481, 250790,
    310134, 103528, 109724, 250806, 321532, 239898, 95169, 324017, 324024, 49023, 96607, 62005, 87707, 32461, 175053, 126911,
    25380, 83825, 85979, 219865, 40400, 175061, 185382, 244745, 292086, 185389, 292093, 253313, 211086, 324030, 132067, 270428,
    304829, 304807, 211097, 324041, 270465, 48003, 142450, 173677, 324047, 63376, 176221, 324056, 233896, 324069, 320946, 82776,
    83479, 311404, 276106, 306933, 324089, 324096, 234364, 289448, 208840, 324112, 323969, 143542, 257408, 324128, 178302, 324135,
    324144, 324153, 83568, 324160, 76731, 102750, 102759, 324167, 311555, 208868, 306942, 107970, 48712, 322022, 40407, 154832,
    152608, 122653, 324176, 324192, 286485, 324185, 6913, 324205, 48012, 203817, 221294, 221267, 68228, 321996, 170138, 54207,
    324215, 324227, 67182, 247383, 324245, 54762, 323908, 324258, 324269, 323997, 324283, 312424, 321800, 166462, 323978, 237503,
    101195, 118658, 324249, 324289, 193461, 323505, 228953, 286950, 67186, 264297, 324295, 324303, 323940, 322899, 323755, 322548,
    324312, 29153, 123077, 67868, 67909, 196157, 283629, 324332, 319882, 45525, 244634, 262038, 324350, 105976, 47934, 324369,
    324373, 324381, 264144, 73593, 73600, 215527, 154541, 51371, 82765, 153951, 34393, 235698, 324388, 233647, 264313, 264319,
    157879, 157885, 324395, 324415, 149741, 271121, 9868, 19419, 279200, 234294, 240583, 297234, 5397, 324406, 120087, 324444,
    324424, 299227, 324461, 324452, 324468, 324434, 324481, 324492, 98922, 253444, 195715, 324500, 204541, 230835, 230854, 320910,
    320917, 211783, 211800, 211820, 198147, 324504, 324511, 324521, 189380, 184281, 321002, 292100, 306947, 101587, 101602, 234959,
    313738, 260093, 258893, 324360, 324531, 324538, 123451, 141814, 232055, 324547, 324557, 166447, 267313, 135994, 232411, 273894,
    135938, 124418, 321014, 198613, 316959, 324566, 153420, 80935, 92720, 302333, 324572, 123840, 70881, 206595, 284640, 324577,
    6920, 324602, 324614, 324628, 324638, 53407, 57126, 324650, 6929, 270545, 281691, 9023, 64328, 181134, 324664, 76454,
    324675, 25592, 201964, 279456, 321839, 279466, 321849, 235918, 324688, 304353, 233870, 324704, 66567, 122389, 200582, 151456,
    223220, 242278, 242288, 242299, 324717, 103859, 136741, 34306, 116014, 324724, 128188, 65690, 116022, 79110, 324732, 110474,
    116033, 25602, 15641, 15653, 15663, 324669, 324711, 324776, 99211, 117276, 310206, 324792, 47942, 14908, 164594, 324806,
    324818, 324850, 324870, 324881, 14914, 324828, 324840, 324860, 87190, 324894, 52861, 324903, 324915, 105983, 305314, 295984,
    324927, 324937, 10275, 149134, 208846, 308948, 321042, 13527, 177973, 13573, 234371, 136227, 176690, 203477, 196357, 185743,
    212760, 314904, 322335, 324945, 324951, 324958, 209521, 279940, 209528, 324989, 324999, 146804, 146851, 324200, 325008, 325013,
    325020, 325029, 325039, 325065, 325074, 108267, 325047, 161068, 325084, 325100, 325114, 325130, 325105, 325121, 325137, 325092,
    284777, 284796, 279312, 325143, 325155, 24235, 142085, 255033, 283503, 98929, 325169, 325178, 325188, 325199, 321307, 325211,
    325231, 325248, 325258, 325221, 283343, 276625, 275421, 276711, 182134, 21789, 247968, 182147, 154463, 71122, 156093, 49413,
    308281, 325269, 142165, 208259, 325284, 45672, 248820, 316674, 325299, 325306, 325315, 325326, 94593, 152711, 306911, 325339,
    199615, 132257, 191643, 293429, 325345, 72455, 293474, 325351, 308074, 57263, 325374, 173781, 317671, 182263, 299990, 316573,
    325393, 316578, 325398, 325406, 66639, 164208, 325423, 325434, 325445, 208631, 314950, 325469, 314958, 299995, 325415, 235568,
    250227, 81724, 282791, 39874, 282836, 39942, 39888, 39955, 39902, 282848, 39969, 39917, 325483, 22151, 319049, 265762,
    83582, 9421, 325508, 278575, 278590, 278602, 325531, 325551, 325588, 38617, 258814, 325611, 38630, 258827, 325622, 308803,
    161235, 175386, 116963, 325634, 325562, 325646, 272797, 72647, 325660, 325671, 165175, 325541, 325697, 325712, 323055, 325727,
    325682, 165185, 116977, 227598, 325520, 305802, 305815, 305844, 318458, 318469, 254390, 24340, 88645, 174390, 325750, 87134,
    93729, 88659, 325763, 174404, 305952, 325777, 325456, 109639, 116990, 259313, 174664, 325796, 312237, 325813, 325839, 183836,
    325826, 325852, 105145, 126447, 200284, 325865, 325881, 325893, 197113, 87163, 93738, 117035, 305935, 258841, 325735, 117081,
    279239, 279266, 116895, 325576, 325599, 325903, 325914, 323769, 323781, 325927, 325939, 77296, 80344, 164216, 216124, 118418,
    84304, 188959, 100784, 92703, 300325, 308112, 300340, 125068, 231009, 232008, 243068, 325952, 325975, 325994, 325984, 326003,
    325963, 125074, 325804, 152763, 306738, 152747, 140206, 278682, 326013, 278717, 104070, 315553, 325477, 277329, 326036, 326046,
    317686, 173792, 325382, 173804, 191862, 96077, 130461, 203487, 262532, 154299, 312339, 322612, 323631, 32823, 32833, 118404,
    118426, 326057, 33193, 326075, 265464, 104036, 251234, 326100, 326110, 183217, 292048, 307194, 164002, 174768, 187508, 292034,
    292240, 326084, 149201, 219799, 148579, 304747, 195224, 121296, 310691, 322400, 326118, 326133, 278332, 326139, 326146, 326153,
    326162, 176230, 272808, 227298, 162168, 70598, 318519, 323616, 289972, 289976, 34994, 326186, 326206, 326233, 326195, 279404,
    202212, 240104, 74163, 14027, 112050, 326255, 326268, 124294, 261256, 321661, 321677, 97862, 202057, 326282, 326295, 326308,
    326321, 231757, 209220, 294192, 153322, 54468, 209228, 122296, 88707, 122304, 310804, 26754, 326331, 326340, 326349, 326359,
    326367, 326376, 326386, 326399, 322725, 41091, 326425, 262482, 183422, 326450, 326461, 326473, 326486, 326497, 200590, 326215,
    298639, 228869, 241801, 326408, 326222, 326244, 176555, 299583, 206, 52231, 326508, 326518, 326529, 319025, 290281, 276257,
    180366, 22577, 264033, 324316, 48486, 326538, 326549, 326561, 269545, 231016, 316931, 326574, 326587, 326598, 322619, 323638,
    322626, 300173, 321316, 169832, 316945, 326579, 241021, 241222, 326610, 326619, 326630, 326638, 48018, 180913, 263526, 172722,
    172732, 326647, 326432, 178418, 318345, 279899, 326439, 276441, 326669, 326679, 89720, 207549, 131521, 25392, 316586, 11532,
    310313, 263656, 275061, 89689, 288588, 326691, 326698, 102809, 326707, 306744, 326712, 326723, 231953, 147886, 326734, 326747,
    220619, 148936, 69799, 200848, 247802, 182606, 49738, 72393, 274652, 274593, 215068, 44845, 264527, 258539, 321785, 326756,
    67816, 326772, 326783, 184654, 284003, 175502, 319458, 244383, 156373, 118444, 174144, 118450, 118468, 207282, 215566, 326794,
    2135, 326803, 308356, 51504, 326764, 65696, 66239, 67603, 288716, 70021, 308374, 326092, 59439, 117993, 324009, 326813,
    318897, 296271, 55167, 326845, 326819, 326863, 326854, 326832, 306790, 326874, 326890, 326902, 6733, 316696, 326175, 326909,
    326920, 129837, 129864, 129907, 129915, 129872, 314793, 92145, 92181, 194916, 323860, 92153, 92189, 48733, 51573, 254183,
    273995, 51386, 326932, 326943, 312440, 99161, 154898, 253659, 282086, 326951, 83616, 161013, 264231, 115222, 326963, 326974,
    161022, 326936, 326956, 127322, 83625, 191364, 225676, 294101, 326986, 326991, 184628, 80092, 123851, 324899, 325053, 325057,
    152732, 326996, 327004, 327011, 327031, 99690, 327021, 327051, 327065, 71728, 107130, 324657, 299598, 327081, 327089, 228168,
    66493, 166178, 228178, 71732, 118759, 147895, 50179, 327097, 251907, 251929, 327108, 293029, 327139, 168606, 24973, 11978,
    11984, 327156, 327187, 154942, 77967, 327102, 327217, 327223, 309926, 324966, 324973, 324981, 281697, 25572, 103997, 327229,
    327239, 327250, 106531, 327259, 174619, 174628, 289740, 327269, 327280, 99481, 147906, 327291, 122685, 198988, 66499, 228184,
    324682, 327317, 231899, 327327, 327300, 327337, 327345, 327355, 327308, 327368, 124866, 135502, 296477, 30194, 316095, 269027,
    190706, 193079, 271917, 308832, 327377, 126862, 327394, 327405, 32201, 84810, 327411, 307678, 327418, 324784, 211012, 230346,
    327429, 327436, 327443, 259965, 304464, 327450, 327458, 128349, 128357, 104461, 327465, 327486, 327490, 322235, 70152, 127123,
    254912, 181000, 104792, 210054, 281806, 281792, 369, 154994, 336, 732, 710, 327497, 319578, 178319, 71101, 327514,
    327521, 155395, 155404, 118038, 120810, 121154, 155109, 191910, 69941, 148749, 163667, 316835, 327530, 281812, 312937, 52623,
    155114, 59710, 191922, 59951, 234471, 319586, 212573, 327503, 148790, 125799, 324696, 324741, 324750, 327509, 281819, 140516,
    140526, 215030, 151715, 154306, 309934, 991, 322876, 49795, 49028, 96612, 7311, 154011, 233904, 324079, 156006, 233912,
    324104, 94191, 110630, 160623, 148601, 110641, 327546, 327558, 327572, 42936, 42945, 270504, 327581, 327593, 270515, 43341,
    318863, 324210, 54212, 327607, 327617, 324221, 324236, 327628, 327637, 327576, 129842, 129881, 129925, 194923, 327646, 136987,
    269771, 327662, 284374, 321028, 262302, 40919, 139712, 319010, 327676, 327683, 143803, 73568, 327695, 327707, 327715, 327699,
    235394, 83493, 83503, 327724, 327731, 327747, 327751, 327756, 237714, 317605, 327760, 172557, 90097, 294680, 100671, 325495,
    94914, 95021, 318653, 327767, 327782, 327791, 29713, 98116, 55684, 219672, 307009, 327801, 32862, 327805, 327812, 327827,
    327837, 327820, 327846, 327855, 222806, 40702, 327879, 217733, 327912, 327895, 327925, 218563, 327936, 327963, 327991, 328026,
    328074, 328084, 328095, 218807, 328117, 218831, 225615, 328140, 328169, 176246, 328193, 264477, 81733, 328213, 16545, 328223,
    218855, 148122, 180277, 93561, 218905, 328253, 328273, 328301, 21281, 328334, 114274, 328362, 73614, 328378, 328413, 328450,
    328482, 328542, 223445, 328589, 328615, 328602, 328632, 322912, 328649, 328679, 264503, 219203, 328716, 328726, 219227, 219251,
    328737, 328768, 76389, 217952, 328796, 266471, 328821, 297944, 309355, 328841, 328864, 217965, 328039, 262453, 328494, 328887,
    328781, 328218, 219452, 328911, 328937, 328924, 328950, 218535, 189316, 222988, 223027, 311417, 219976, 328832, 328971, 70839,
    215547, 328992, 329003, 329014, 329061, 329072, 227759, 228583, 329084, 221072, 329118, 329128, 218197, 220901, 40173, 144815,
    263057, 56699, 55806, 316246, 315311, 224151, 329140, 329155, 329164, 329174, 329202, 329235, 266560, 307297, 65547, 329023,
    69218, 69280, 329147, 329284, 329305, 329316, 329328, 233745, 329368, 144837, 77490, 329397, 329419, 159331, 288067, 264575,
    323527, 328184, 329438, 329452, 89467, 12691, 89994, 329475, 329488, 93443, 219714, 329502, 287150, 287164, 329521, 329560,
    93588, 93604, 328263, 314587, 93621, 94564, 329338, 95563, 178191, 79801, 329597, 329611, 48965, 48977, 328504, 218462,
    311925, 221125, 329624, 329647, 103944, 101734, 329674, 329697, 329724, 329185, 329747, 329757, 264679, 221083, 329768, 329785,
    43897, 328875, 294134, 220029, 220058, 329802, 220154, 220176, 159685, 329827, 329857, 329886, 220235, 134248, 224179, 221803,
    88825, 329907, 225009, 329921, 329935, 264830, 328981, 211396, 112833, 226404, 329093, 114005, 170657, 288972, 220356, 145558,
    1891, 329959, 329985, 222384, 229509, 330007, 330049, 264860, 218261, 330077, 220659, 182439, 218210, 147236, 330022, 330101,
    330127, 330158, 330180, 120978, 134347, 104663, 67524, 171322, 171338, 123333, 264941, 125574, 22455, 330202, 22470, 330217,
    47793, 188411, 78785, 188419, 330232, 330245, 36873, 244187, 36886, 329348, 127921, 330260, 330269, 330279, 330292, 132085,
    330308, 144488, 330338, 221133, 328347, 220802, 328105, 38309, 38322, 215705, 330170, 221498, 330372, 330388, 330450, 330404,
    330474, 263117, 264407, 213820, 255209, 330496, 330527, 82463, 330538, 69179, 244233, 330548, 330561, 288399, 288424, 330585,
    330604, 330624, 101382, 322461, 140595, 70000, 330648, 309983, 330595, 330672, 330699, 330709, 55774, 101393, 253664, 311433,
    330505, 311443, 330515, 330723, 330754, 330763, 317392, 188732, 224604, 330776, 184077, 89702, 330657, 317727, 264421, 317246,
    330614, 330636, 309563, 330733, 68723, 330801, 330811, 279838, 314275, 330743, 317520, 330788, 110701, 31659, 330822, 320537,
    184425, 315027, 68733, 258269, 324118, 317086, 317293, 14262, 330575, 66892, 330863, 330884, 330907, 330928, 330947, 330958,
    155192, 221871, 221678, 221707, 330348, 330969, 330994, 331018, 331041, 331066, 11239, 272038, 331090, 221897, 330113, 331114,
    331139, 330980, 328753, 331162, 221956, 330318, 220498, 329573, 222145, 331186, 169745, 243862, 331216, 328052, 140175, 270003,
    237849, 237860, 300431, 331228, 331243, 331253, 328511, 169432, 331273, 331303, 105318, 181048, 331323, 331312, 217403, 329195,
    132883, 12460, 159339, 313672, 147737, 147744, 292274, 29269, 44687, 297535, 222727, 331339, 331350, 331363, 331385, 222837,
    222961, 310015, 327865, 331411, 65556, 297614, 161200, 12701, 227770, 240145, 311043, 331438, 260858, 331396, 331469, 265013,
    331499, 44668, 218787, 19717, 331522, 331534, 19727, 135421, 328313, 328285, 221594, 223176, 223264, 255668, 268865, 304176,
    331546, 331569, 331580, 331603, 331202, 220673, 331633, 331658, 319077, 239305, 331683, 327977, 331103, 331006, 265076, 265090,
    331175, 331702, 223567, 132760, 20767, 184365, 331784, 331805, 223146, 173815, 223885, 315322, 316350, 331819, 329408, 331843,
    270010, 4832, 289076, 144515, 331861, 331886, 144527, 331873, 331902, 41765, 223710, 217856, 176339, 203591, 81576, 192519,
    223849, 331919, 331932, 329531, 331959, 307770, 331976, 331992, 218509, 219123, 262105, 221303, 331794, 217296, 329873, 265129,
    265140, 181721, 81667, 181876, 215819, 329246, 332006, 332017, 332051, 332061, 171736, 329707, 332072, 184937, 328464, 260388,
    218879, 223342, 223458, 219385, 50331, 243488, 211408, 224706, 332092, 191506, 44593, 104232, 79814, 328521, 328530, 329736,
    250829, 315905, 214845, 332126, 332142, 109449, 109461, 194163, 195811, 224735, 222399, 329357, 331128, 196788, 196813, 145702,
    144387, 332156, 322949, 297551, 322926, 332184, 332207, 47801, 332230, 332242, 210744, 210753, 332255, 332273, 148862, 223804,
    332295, 332317, 224582, 299304, 225396, 225458, 225408, 219322, 332349, 214985, 206136, 328390, 297275, 332284, 70850, 137214,
    309407, 332266, 332367, 332381, 332395, 2615, 147197, 135743, 212330, 136154, 332326, 332413, 332419, 257520, 261024, 117498,
    220760, 114678, 331058, 332426, 332444, 332455, 20725, 91358, 114726, 240932, 331484, 217709, 315665, 315687, 331262, 61714,
    331619, 225805, 331945, 329946, 332467, 332484, 332501, 23860, 332528, 260891, 260902, 332560, 332572, 315701, 260872, 265364,
    305352, 316971, 269964, 139424, 332586, 115922, 329258, 332514, 315712, 332620, 135632, 176489, 266922, 265408, 270985, 265418,
    270995, 332642, 332652, 19738, 309588, 309607, 332663, 332681, 239881, 231819, 332692, 332700, 6897, 269832, 257765, 191316,
    119985, 332708, 332721, 89069, 120041, 328325, 223130, 331812, 221352, 331279, 224914, 40687, 282599, 332736, 332759, 214931,
    214941, 261037, 316314, 316373, 332801, 332816, 332826, 329292, 329430, 332846, 269246, 332861, 329032, 332882, 332893, 215156,
    218276, 332905, 238780, 214901, 225023, 332923, 23780, 214911, 328004, 328015, 85687, 240946, 332943, 328554, 332959, 328564,
    332969, 332980, 332993, 218223, 218243, 148171, 223369, 241307, 271958, 304634, 241897, 240956, 332953, 265481, 329686, 333006,
    333016, 145163, 269253, 329042, 215311, 333027, 333035, 333044, 332194, 151003, 244576, 142398, 330036, 330064, 333067, 295002,
    333089, 333098, 158322, 291418, 333108, 333116, 333127, 94574, 272545, 333138, 329214, 309514, 249754, 249766, 268876, 268886,
    215881, 215220, 265557, 333177, 333190, 265583, 265593, 265606, 333203, 327117, 333213, 330685, 333151, 252345, 252370, 265673,
    185459, 268127, 206185, 333235, 30568, 216092, 329899, 333245, 333253, 333266, 258504, 258516, 332872, 216232, 328151, 265694,
    330285, 265714, 333225, 329223, 222747, 261045, 2837, 13544, 333279, 333293, 333307, 333319, 333328, 333341, 185776, 298417,
    333355, 272554, 120988, 121001, 121012, 225123, 333375, 327952, 333400, 333421, 218589, 333439, 225242, 103430, 147261, 332165,
    266525, 333461, 266537, 333473, 331425, 250210, 207788, 330834, 333485, 272911, 328127, 20835, 333527, 268894, 333544, 333559,
    19745, 275487, 216566, 333575, 333589, 147176, 276885, 276898, 333603, 216971, 333625, 333648, 333661, 297642, 265856, 333674,
    330089, 332029, 332766, 330485, 333053, 282580, 332600, 306480, 331448, 329377, 329388, 145463, 289087, 284760, 191386, 30005,
    329974, 333613, 333699, 333711, 84670, 333732, 328401, 216542, 333760, 333776, 333500, 330849, 333513, 159699, 333790, 218602,
    333812, 333828, 189264, 331458, 218444, 333845, 219414, 134836, 333873, 333882, 333891, 258428, 333905, 298430, 220109, 331510,
    292379, 222049, 42608, 222067, 329584, 217613, 29276, 333917, 332934, 332175, 294541, 332773, 146389, 146399, 328694, 328706,
    221148, 36760, 226280, 332104, 328061, 332115, 169438, 63068, 333941, 328424, 333964, 333994, 333979, 334007, 229327, 242487,
    229356, 229286, 127789, 333802, 334020, 229299, 334057, 334083, 268357, 334070, 334104, 334117, 334131, 334143, 229427, 171695,
    74921, 223470, 279498, 330421, 326025, 330463, 330435, 331079, 168685, 333637, 90004, 216908, 265978, 334156, 333928, 128305,
    216921, 229481, 229539, 328439, 240495, 297254, 225891, 148339, 330361, 334176, 147850, 132710, 334095, 136511, 266180, 331670,
    185499, 229688, 331151, 266053, 334166, 298722, 334196, 334207, 311263, 298731, 215838, 215845, 249776, 249786, 334220, 282800,
    332041, 186439, 66127, 147217, 222412, 269270, 131290, 225540, 331714, 334234, 329816, 334244, 334255, 289029, 288986, 225470,
    334275, 225502, 334283, 297563, 334294, 144676, 127800, 146657, 146685, 148461, 269087, 148470, 269096, 329636, 332333, 334306,
    332781, 329512, 304642, 334336, 334358, 334264, 334388, 266109, 329051, 332220, 228698, 334416, 172898, 333412, 221092, 328475,
    333432, 266134, 139228, 334432, 334465, 93453, 88042, 329542, 76402, 315334, 330329, 334488, 334497, 334506, 334515, 334524,
    144947, 334542, 334549, 334534, 329272, 63086, 148144, 20804, 220133, 68537, 334557, 146317, 146366, 334574, 334588, 334603,
    225904, 225866, 225914, 334187, 77012, 329841, 334632, 329466, 330938, 174196, 331998, 334658, 289097, 334425, 238242, 2999,
    221451, 222758, 334675, 334689, 331694, 330192, 334703, 334713, 260104, 331721, 95217, 95228, 334612, 287126, 334666, 226015,
    314697, 334622, 334722, 328854, 331730, 298872, 331983, 334744, 334757, 331644, 238252, 238262, 332081, 22495, 331235, 329997,
    317008, 331833, 333688, 222500, 334475, 226287, 330142, 3224, 226314, 223089, 219073, 330896, 223103, 255684, 314707, 328663,
    332809, 142456, 219724, 226496, 329659, 331591, 226556, 334770, 334783, 332541, 304921, 315236, 331030, 266302, 266008, 334796,
    26011, 334808, 103444, 144587, 328575, 332745, 334822, 334837, 266328, 266343, 266358, 320590, 332837, 314717, 320770, 297873,
    146228, 332792, 331968, 314727, 334852, 334317, 266385, 221463, 332306, 328900, 332631, 221405, 333450, 334399, 334871, 322960,
    244871, 323536, 334033, 332403, 334044, 323563, 252878, 243867, 329299, 329553, 331854, 332854, 270021, 225270, 334375, 287674,
    334349, 334888, 203826, 214854, 219508, 333365, 203935, 218615, 328809, 328159, 332550, 334861, 12197, 221101, 276635, 266441,
    334914, 334926, 266484, 220702, 334939, 334950, 334408, 334880, 330298, 276645, 276652, 328202, 334962, 239223, 334975, 334993,
    335007, 327129, 334326, 41544, 335016, 335029, 335044, 335060, 335076, 334446, 334455, 228201, 316330, 221474, 222770, 333861,
    331739, 328670, 181886, 228594, 329105, 335090, 334732, 93519, 332434, 335099, 333166, 331749, 331762, 333387, 333723, 328231,
    318873, 331775, 228778, 335109, 335122, 330919, 332358, 221366, 50341, 221380, 331291, 283567, 328242, 315345, 334896, 334906,
    221314, 335135, 335141, 335150, 16232, 258009, 44791, 104241, 333080, 228739, 146337, 334227, 223520, 184110, 335160, 214957,
    332610, 326066, 217867, 334648, 86032, 219151, 139717, 330875, 331221, 335169, 270632, 314002, 150084, 9241, 39202, 218375,
    335192, 95504, 294949, 31861, 227132, 5902, 5872, 246747, 208576, 324585, 22613, 324592, 58505, 335202, 272849, 335213,
    335224, 240860, 166190, 232797, 334566, 335239, 166217, 166239, 295560, 34937, 37018, 73441, 208659, 292865, 138897, 140182,
    335232, 145362, 114330, 19819, 335256, 335264, 36027, 276148, 157809, 272828, 298103, 335195, 335273, 45066, 335288, 282445,
    39532, 271761, 285092, 335301, 335311, 335321, 85356, 45074, 247623, 235406, 287906, 282409, 277524, 271342, 315575, 327196,
    327165, 327175, 327205, 89386, 89352, 277480, 335325, 42357, 21917, 21920, 306616, 43919, 188614, 335330, 306626, 335345,
    335351, 163596, 188578, 43926, 188621, 335337, 306637, 171211, 304961, 335375, 335388, 335407, 228214, 171248, 151176, 308476,
    335426, 310176, 335432, 300582, 75993, 257286, 170672, 318700, 335439, 335442, 318631, 335447, 335382, 161412, 335459, 335395,
    335418, 161421, 335468, 172522, 175283, 250339, 335477, 335485, 61943, 312967, 171470, 312992, 335495, 172529, 175290, 250346,
    4008, 335523, 335527, 335025, 335040, 335056, 335072, 335086, 327779, 335532, 335538, 335545, 327689, 335357, 335363, 335369,
    335569, 335575, 335586, 161469, 1876, 335598, 335609, 216279, 225190, 335621, 335629, 335638, 183071, 335648, 207668, 219052,
    335669, 335684, 335694, 161519, 254969, 161477, 210908, 335677, 16285, 103065, 293725, 299935, 284439, 16294, 103074, 307028,
    330382, 284459, 67774, 111173, 319486, 65107, 118558, 57545, 335705, 335717, 335725, 192400, 335733, 335750, 335765, 189814,
    189823, 335778, 107259, 107270, 105923, 335795, 335804, 141638, 199455, 335741, 335710, 189832, 335786, 189838, 189851, 143868,
    335814, 228363, 31486, 287256, 335832, 335841, 335849, 133492, 117843, 117848, 236554, 85312, 335857, 335866, 335877, 335887,
    61495, 335897, 335914, 181859, 176822, 111126, 153174, 161499, 53102, 113979, 11840, 61814, 231183, 231192, 335923, 168482,
    335936, 335995, 335946, 168492, 336015, 191089, 336005, 105157, 253778, 335955, 205069, 335929, 168504, 335964, 168515, 191099,
    335975, 105166, 253787, 335985, 205082, 75312, 257381, 240307, 331557, 336028, 336062, 336040, 144637, 336053, 260078, 336079,
    336091, 336104, 93323, 149427, 336115, 336124, 336133, 38893, 149380, 297436, 336143, 234811, 273599, 336161, 336169, 336177,
    304582, 314677, 336071, 336192, 83633, 287938, 287955, 336203, 336216, 280388, 9934, 161664, 177928, 73923, 164081, 326947,
    149569, 149575, 270615, 272528, 150585, 272643, 64278, 336228, 336233, 325789, 310697, 336238, 336242, 336259, 336291, 336267,
    336280, 336299, 336310, 336321, 336252, 161273, 177328, 178519, 184128, 89399, 336330, 191488, 263043, 336341, 336351, 336358,
    336374, 336393, 336398, 326414, 336404, 336411, 336335, 336419, 336434, 336445, 336426, 336346, 72832, 248786, 78948, 248831,
    248795, 248838, 173345, 336456, 336463, 336473, 42052, 336486, 297406, 192250, 336500, 336508, 336515, 298190, 169861, 255224,
    336523, 280615, 284815, 336530, 336367, 64295, 336548, 44146, 2203, 64300, 17296, 9441, 336561, 336572, 193894, 12022,
    41792, 12073, 41804, 242913, 336583, 336590, 335295, 336597, 336602, 336608, 189901, 189909, 80513, 171478, 313000, 171489,
    313011, 336616, 321957, 184918, 198931, 336632, 291441, 198872, 198940, 73357, 73369, 336651, 112499, 107281, 336665, 107294,
    42623, 336682, 336693, 336718, 336705, 27489, 100034, 303096, 100044, 336185, 336736, 336747, 336765, 229250, 49272, 313201,
    336781, 336791, 336802, 336815, 258602, 336151, 235186, 254246, 284701, 298249, 122462, 122482, 8005, 336641, 166198, 279358,
    335247, 166251, 166263, 336840, 336853, 149629, 336875, 336624, 336887, 214458, 61419, 336902, 336911, 129607, 207717, 336196,
    336922, 309574, 336940, 291141, 27218, 300382, 300874, 103749, 214654, 214663, 336955, 336967, 269505, 304849, 336959, 336971,
    336984, 15203, 26397, 26498, 169104, 208074, 309903, 169926, 308980, 23097, 109295, 336995, 337010, 337037, 337016, 152428,
    283081, 337057, 337026, 337044, 271380, 337000, 337075, 337082, 337090, 275922, 186553, 337107, 337117, 242975, 49071, 111771,
    168780, 284824, 74966, 155007, 171216, 337125, 171227, 337136, 318428, 337147, 288361, 326658, 337161, 108777, 290204, 337180,
    290214, 337190, 18303, 108785, 109302, 337201, 155600, 155610, 26145, 26162, 172951, 210986, 303294, 243731, 179176, 272444,
    337222, 337245, 155805, 163155, 163179, 155811, 163161, 163185, 207261, 306804, 158860, 158871, 139726, 242737, 242790, 262311,
    45681, 202578, 46455, 304101, 262277, 337262, 337277, 206688, 337286, 337292, 337300, 163168, 337312, 336381, 53115, 269812,
    4081, 30074, 232871, 297740, 298016, 139793, 298199, 7479, 337329, 7452, 337357, 337342, 82636, 7463, 337368, 337377,
    337384, 337393, 337322, 337403, 100088, 337421, 139831, 289008, 269116, 133206, 238328, 177338, 124144, 178528, 291304, 178287,
    178336, 90617, 90637, 337428, 80734, 299562, 142976, 262683, 20977, 263396, 298784, 336658, 291148, 337437, 291155, 335589,
    337444, 337450, 337470, 198958, 247032, 286340, 247773, 337488, 336881, 286712, 94759, 252483, 337504, 115194, 291164, 337511,
    337460, 337479, 337524, 337534, 7072, 336539, 337495, 120465, 120484, 60043, 97194, 164643, 228141, 313780, 195124, 106327,
    106337, 337544, 337568, 337555, 337580, 337594, 195131, 320311, 337604, 337616, 305189, 337627, 16770, 215433, 225984, 337639,
    337648, 281835, 59061, 337656, 40002, 268796, 268810, 165014, 337668, 165025, 337679, 337690, 124594, 242539, 25924, 25886,
    336893, 25934, 222008, 337710, 337207, 337214, 119235, 143487, 337699, 194259, 194268, 308491, 7081, 89531, 303115, 337065,
    337724, 337735, 197751, 226772, 98963, 25863, 204219, 59067, 337662, 272014, 309428, 335503, 309438, 309447, 335513, 62432,
    96279, 326125, 248044, 227580, 312186, 29586, 72159, 146267, 318882, 161484, 267583, 297075, 134703, 200738, 228270, 96346,
    274171, 44260, 171287, 178209, 168528, 191110, 169647, 333749, 168542, 191122, 336553, 314803, 160964, 286357, 189990, 35580,
    337746, 337756, 36535, 67197, 76265, 226821, 337766, 337777, 188336, 117698, 324757, 188349, 337790, 322528, 333534, 337800,
    165459, 319631, 68454, 165470, 232421, 127998, 245008, 106492, 106541, 106548, 49375, 160945, 313292, 160951, 49381, 177652,
    157600, 294045, 157659, 198800, 157673, 117237, 317839, 146277, 159413, 327669, 336674, 261862, 277550, 337810, 337822, 336744,
    127472, 337836, 156709, 127483, 337847, 104671, 156388, 283576, 336758, 285768, 337858, 104717, 104727, 285779, 337869, 337880,
    337891, 248267, 287428, 287438, 337899, 337906, 337913, 267879, 196311, 77177, 95059, 122366, 273395, 105990, 278038, 85624,
    337927, 57292, 337931, 337938, 337952, 337965, 337982, 144081, 337959, 337974, 337995, 338018, 338025, 286641, 230724, 236532,
    236540, 144087, 130209, 338033, 274460, 338042, 338067, 338054, 338079, 338007, 307343, 338092, 99966, 99976, 336945, 338098,
    306578, 14408, 337254, 338108, 289116, 35704, 338116, 211868, 338123, 316301, 82298, 276179, 184509, 338144, 276184, 76682,
    275614, 154263, 59189, 151515, 216431, 338158, 48426, 59222, 338181, 274748, 304556, 337945, 281208, 277124, 277131, 274022,
    51520, 250594, 293734, 299457, 166539, 250632, 256149, 54102, 179115, 249661, 324799, 38217, 38225, 7687, 262144, 96087,
    262190, 3904, 135223, 258439, 289311, 1904, 240630, 159968, 90604, 103596, 252289, 338195, 338208, 120325, 277009, 269513,
    94341, 204906, 249264, 34320, 205695, 81049, 322984, 338219, 254518, 13676, 205670, 338228, 10703, 75414, 143665, 338235,
    200129, 200139, 143674, 338244, 55555, 249244, 338253, 338260, 336810, 336825, 338269, 338278, 336834, 234458, 239234, 16669,
    237992, 238073, 299258, 238078, 50641, 202733, 313824, 50651, 202743, 181014, 238107, 238043, 238088, 337916, 154520, 181024,
    50660, 238055, 62596, 338287, 73519, 86710, 324340, 50119, 195774, 242050, 323660, 37460, 41004, 79786, 271662, 314262,
    331376, 323666, 50125, 140191, 338300, 158466, 313446, 154803, 338307, 243836, 323954, 70200, 243738, 338315, 17743, 225945,
    337232, 211128, 338323, 70210, 201120, 277530, 338332, 338352, 338375, 338363, 338389, 316551, 338404, 336929, 123793, 256698,
    174296, 152404, 338342, 34856, 65904, 65166, 293535, 293556, 293567, 304766, 50676, 149472, 103867, 76413, 338416, 295960,
    338293, 237223, 327471, 327478, 71166, 127675, 181057, 331334, 124392, 174346, 182238, 183081, 338425, 207679, 183099, 335660,
    127705, 124401, 52131, 311633, 338437, 338456, 338443, 60452, 55997, 218066, 338451, 338462, 144254, 175441, 299856, 232025,
    278940, 338467, 307914, 299418, 116324, 338483, 240177, 41240, 143496, 212621, 323473, 133303, 151217, 133261, 206791, 14067,
    35261, 165958, 35488, 71774, 103757, 103762, 230556, 170145, 68237, 44443, 170153, 111936, 335280, 101536, 101549, 338492,
    338514, 338527, 335902, 338502, 227741, 250601, 338431, 338544, 338551, 164256, 335919, 338562, 236183, 236190, 338486, 236196,
    338570, 338574, 216447, 60541, 151354, 207339, 66904, 205862, 143551, 186629, 60545, 60552, 123904, 91834, 150487, 277536,
    178962, 77457, 338581, 338600, 238707, 205802, 338588, 338607, 240996, 240876, 105947, 118837, 199198, 159349, 338617, 123595,
    60884, 123162, 123603, 123610, 338628, 338662, 199009, 56214, 161622, 319473, 338678, 207741, 338687, 82319, 82343, 338670,
    82325, 182614, 338188, 59866, 245260, 253421, 268512, 338698, 338708, 338726, 338735, 338743, 338755, 338717, 236124, 198488,
    20635, 53837, 318931, 338165, 338764, 267885, 196317, 193308, 47283, 338171, 50453, 56899, 281341, 281347, 29288, 234915,
    29297, 316521, 338780, 104929, 58204, 58229, 292836, 50194, 58268, 338798, 281727, 338806, 39180, 39189, 295918, 291078,
    338818, 319548, 32933, 84645, 32941, 217452, 32954, 155742, 189140, 338832, 58766, 55641, 187196, 338837, 187203, 182619,
    237598, 338847, 180580, 319614, 230442, 338858, 319560, 182187, 338867, 338878, 86691, 322071, 157698, 98662, 338852, 202029,
    337099, 49124, 52834, 52815, 49134, 338892, 338900, 259696, 278101, 195953, 323419, 228960, 207397, 254535, 62541, 250371,
    327041, 210618, 254560, 275442, 338907, 338926, 338916, 202265, 328372, 332343, 332686, 205583, 338788, 319385, 231667, 126874,
    208999, 338937, 50458, 338176, 338772, 48436, 51968, 46897, 231763, 338950, 231775, 338962, 38518, 50960, 106448, 106499,
    127664, 106556, 336867, 58776, 58798, 338825, 338974, 338987, 321244, 33504, 309414, 322643, 242799, 262320, 309334, 83424,
    83431, 199284, 210823, 339000, 339015, 179723, 339025, 239006, 61989, 312834, 73717, 73786, 164090, 327385, 339048, 31176,
    339063, 339071, 71746, 338475, 202936, 186636, 186643, 335553, 335562, 339080, 339056, 231504, 25485, 314389, 339090, 309701,
    339107, 123465, 339008, 53006, 328963, 195523, 177347, 188038, 339141, 186450, 188045, 339148, 186461, 329716, 196865, 286052,
    337306, 241366, 122311, 41968, 96326, 339156, 339164, 124199, 339170, 209467, 250607, 327536, 339185, 191044, 307701, 26679,
    339198, 339209, 26691, 64346, 24825, 250642, 324768, 339176, 12971, 339219, 339226, 325239, 339237, 339248, 105716, 16780,
    64451, 321965, 337398, 246858, 337411, 339259, 66943, 26214, 46045, 177103, 339270, 339279, 63112, 63217, 279073, 7584,
    337154, 177618, 337172, 154097, 290367, 52766, 69905, 95341, 291673, 339290, 187552, 339298, 339265, 290956, 293035, 293050,
    339305, 339313, 321286, 199301, 316849, 338977, 46470, 312399, 339319, 205390, 217243, 235167, 338641, 312406, 339326, 338650,
    42370, 38252, 139571, 339335, 307034, 339344, 339350, 339359, 278138, 85628, 55031, 161903, 338994, 291837, 94450, 146296,
    295072, 222520, 295330, 19864, 339368, 304716, 308639, 26282, 26317, 134938, 339374, 264107, 308001, 162482, 41279, 224260,
    243746, 224268, 114101, 293898, 339380, 94459, 246196, 94466, 308648, 308654, 339085, 155747, 85984, 106004, 307819, 298899,
    339394, 339400, 123240, 339406, 138360, 339416, 53014, 249369, 339423, 88927, 236728, 157610, 339442, 339450, 245269, 327148,
    339432, 85532, 195244, 253843, 94147, 280408, 158334, 280418, 339459, 339469, 176907, 335454, 318196, 249298, 174779, 10471,
    225319, 174789, 155673, 37740, 298906, 6879, 339481, 68180, 87554, 199676, 339495, 339517, 197176, 339507, 339527, 339537,
    185400, 199817, 310276, 339558, 339575, 277247, 338135, 339547, 339566, 339583, 33040, 155757, 339486, 339592, 33047, 317824,
    178967, 181952, 339603, 339608, 339623, 132291, 153098, 339615, 339630, 9634, 182564, 236664, 339638, 9641, 182571, 339645,
    338944, 149641, 130359, 243341, 260113, 321971, 169933, 168698, 339651, 339659, 55940, 339670, 8879, 289283, 107353, 188658,
    339678, 339695, 339686, 339714, 339706, 283096, 339723, 308228, 120050, 156231, 194582, 212139, 259624, 156321, 62049, 305077,
    119995, 194588, 225526, 314352, 124235, 124244, 336730, 86256, 31711, 147787, 189216, 220635, 148946, 333954, 148956, 189224,
    339729, 339735, 55184, 339744, 10976, 195838, 10937, 55194, 339754, 139296, 335823, 139731, 223065, 139555, 212635, 314441,
    339765, 307048, 161994, 327541, 339777, 339790, 4189, 102969, 211663, 339782, 339795, 319339, 339803, 339812, 94088, 339190,
    339771, 149457, 312877, 339836, 292884, 314357, 197816, 314365, 314397, 339098, 339114, 339124, 115959, 339843, 124328, 124359,
    189024, 291317, 332672, 339849, 335180, 123355, 339858, 86809, 70221, 42700, 314373, 115975, 115983, 339132, 304386, 72549,
    269067, 166597, 332914, 219182, 220782, 339821, 339827, 256778, 154188, 193231, 339866, 327739, 339876, 151226, 338566, 339885,
    339896, 339901, 339032, 339039, 66951, 339888, 338623, 200083, 339387, 20038, 245150, 245161, 339907, 1667, 1688, 1695,
    1703, 1712, 687, 1720, 1727, 1735, 1744, 339926, 5025, 207873, 207897, 339929, 339935, 339945, 295, 93311,
    343, 389, 309652, 441, 679, 309892, 1000, 1012, 1039, 1046, 1054, 1063, 1071, 1078, 1086, 1095,
    9219, 489, 533, 582, 635, 717, 745, 752, 760, 769, 811, 777, 784, 792, 801, 840,
    870, 878, 887, 897, 906, 914, 923, 933, 339955, 240720, 339962, 248599, 109096, 339968, 339978, 230589,
    76978, 230731, 230737, 230744, 230750, 4864, 293953, 93277, 267040, 339990, 267045, 339995, 231234, 80349, 87, 127951,
    273193, 293492, 110980, 325363, 340002, 340011, 340020, 323605, 254476, 340029, 340037, 246931, 131227, 153878, 340045, 340062,
    340088, 340101, 340048, 47946, 26511, 243581, 49655, 340114, 49667, 340124, 157891, 157897, 149718, 308407, 52089, 195972,
    327651, 339916, 340131, 340139, 53925, 166388, 173046, 340147, 340156, 230757, 230762, 340065, 340166, 213347, 188165, 193855,
    271943, 319104, 230211, 76651, 76655, 340172, 340069, 76661, 340180, 340189, 56817, 340204, 340210, 340197, 121207, 241719,
    76984, 69189, 337719, 340219, 340223, 340053, 340078, 340091, 96617, 340230, 232994, 340238, 336772, 233000, 189532, 340104,
    235438, 340244, 340252, 232293, 340261, 232326, 232302, 340270, 136004, 300673, 340282, 78911, 340302, 142148, 113257, 340317,
    340325, 132535, 257431, 257437, 293971, 70157, 75774, 196324, 336024, 340333, 340341, 302062, 340351, 160920, 302271, 340373,
    340390, 340416, 340376, 45116, 155985, 305253, 154038, 44316, 340429, 340393, 340292, 7878, 66575, 312321, 16384, 249344,
    100217, 282241, 49674, 242157, 49682, 282283, 307737, 340381, 340402, 340419, 340412, 340437, 45120, 340306, 340443, 340310,
    340452, 340461, 52240, 298617, 338151, 90192, 340469, 155848, 230219, 309735, 163192, 212593, 340477, 213555, 340481, 17731,
    69021, 302280, 340488, 100897, 100904, 45035, 340474, 309313, 78918, 306550, 43668, 340501, 43674, 340507, 340514, 340524,
    340530, 340519, 340537, 340544, 252031, 340551, 311720, 338932, 340565, 340363, 9152, 340498, 340578, 340581, 121222, 300566,
    340588, 340599, 95369, 340612, 340624, 340637, 340648, 19298, 288925, 340661, 340671,
];
