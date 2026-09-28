//! Colour combinations put together by eye, for the theme builder to offer
//! beside the ones it works out by rule.
//!
//! A harmony is arithmetic on a hue circle, and everything it proposes has
//! the evenness of arithmetic. A designer's combinations do not: they pair a
//! dull olive with a sharp vermilion because it works and not because the
//! two stand a hundred and twenty degrees apart. So the builder offers both,
//! and these are the second kind.
//!
//! They are the three- and four-colour combinations of Sanzo Wada's
//! "A Dictionary of Colour Combinations" (Wada, 1883-1967), numbers 121 to
//! 348 of the book's 348. The two-colour combinations are left out on
//! purpose: a theme has three accent families and a page tint, so a pair
//! names half of a theme and the builder would be inventing the rest under
//! a borrowed name. Three colours are the three families exactly, and four
//! are the families and the page.
//!
//! Nothing here is shown as it stands. The builder finds the combinations
//! that hold a colour near the one the person picked, rewrites each as
//! offsets from that colour, and lays the offsets back down on the pick --
//! so what is offered always starts from the person's own colour, and the
//! values below are where the offsets come from rather than a palette
//! anybody is handed. They are approximations in any case: the book is
//! printed in inks and these are conversions of them.
//!
//! The values come from a data set compiled by Dain M. Blodorn Kim and
//! corrected by Matt DesLauriers, used under the licence it carries:
//!
//! The MIT License (MIT)
//! Copyright (c) 2020 Matt DesLauriers
//!
//! Permission is hereby granted, free of charge, to any person obtaining a
//! copy of this software and associated documentation files (the
//! "Software"), to deal in the Software without restriction, including
//! without limitation the rights to use, copy, modify, merge, publish,
//! distribute, sublicense, and/or sell copies of the Software, and to permit
//! persons to whom the Software is furnished to do so, subject to the
//! following conditions:
//!
//! The above copyright notice and this permission notice shall be included
//! in all copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
//! OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
//! MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN
//! NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
//! DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
//! OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE
//! USE OR OTHER DEALINGS IN THE SOFTWARE.

/// Every combination, as `0xRRGGBBAA`, in the book's order and under the
/// book's numbers: the comment on each row is the number it carries there.
pub const COMBINATIONS: &[&[u32]] = &[
    &[0x7C4226FF, 0xD8A37BFF, 0x555832FF], // 121
    &[0xCC1236FF, 0xFDBF68FF, 0x00978DFF], // 122
    &[0xF58E84FF, 0xF8ED43FF, 0x7A4456FF], // 123
    &[0x802626FF, 0xE2B540FF, 0xA6A159FF], // 124
    &[0xD1B0A7FF, 0x0093A5FF, 0x40456AFF], // 125
    &[0xEBD3A2FF, 0xE2B540FF, 0x1C4286FF], // 126
    &[0xFDC57EFF, 0x648F7BFF, 0x66629CFF], // 127
    &[0xF8B6BAFF, 0x62C6BFFF, 0x9A72AAFF], // 128
    &[0xFFDD00FF, 0xBC892BFF, 0x97ACC8FF], // 129
    &[0xBB7125FF, 0xA62C37FF, 0x4F4086FF], // 130
    &[0xBB7125FF, 0xD96629FF, 0x00939BFF], // 131
    &[0xF5ECC2FF, 0xF3A257FF, 0xB09F36FF], // 132
    &[0x82241FFF, 0xB09F36FF, 0x00B49BFF], // 133
    &[0xF37F94FF, 0x9A72AAFF, 0x59256AFF], // 134
    &[0xF5ECC2FF, 0x437742FF, 0x97ACC8FF], // 135
    &[0xCB2F43FF, 0x009465FF, 0x59256AFF], // 136
    &[0xC55347FF, 0xFDC57EFF, 0x648F7BFF], // 137
    &[0xF3A257FF, 0xF8ED43FF, 0x62C6BFFF], // 138
    &[0x97ACC8FF, 0x051230FF, 0xB6BFC1FF], // 139
    &[0xF3A257FF, 0x007190FF, 0x34454CFF], // 140
    &[0xF37420FF, 0xAFD472FF, 0x12354EFF], // 141
    &[0xA94151FF, 0xC19F2CFF, 0x97ACC8FF], // 142
    &[0x006EB8FF, 0xB984AFFF, 0xA1A39AFF], // 143
    &[0xB73F74FF, 0xF37420FF, 0x111314FF], // 144
    &[0x7C4226FF, 0xB2B73EFF, 0x1E0E3FFF], // 145
    &[0xBC892BFF, 0x635A3AFF, 0x1A7444FF], // 146
    &[0xF27291FF, 0x82241FFF, 0xB5DECCFF], // 147
    &[0xD6B43EFF, 0xFCB315FF, 0x0093A5FF], // 148
    &[0xD6B43EFF, 0xF37420FF, 0x112F2CFF], // 149
    &[0xFDD4BDFF, 0xB2B73EFF, 0xB4CDC2FF], // 150
    &[0xF5ECC2FF, 0xF99D1BFF, 0x064F6EFF], // 151
    &[0xC55347FF, 0x793327FF, 0xA5C8D1FF], // 152
    &[0xF37F94FF, 0xFCB315FF, 0xB2B73EFF], // 153
    &[0xCC1236FF, 0xFFF200FF, 0x006EB8FF], // 154
    &[0xEB5324FF, 0x00978DFF, 0x051230FF], // 155
    &[0xD6B43EFF, 0x96D1AAFF, 0x4F4086FF], // 156
    &[0x7D133AFF, 0xD6B43EFF, 0x5A82B3FF], // 157
    &[0xF8ED43FF, 0xC27544FF, 0x87C540FF], // 158
    &[0xBC892BFF, 0x78CDD0FF, 0xB5B1D8FF], // 159
    &[0xC19F2CFF, 0x71502FFF, 0x547076FF], // 160
    &[0x7C4226FF, 0xEEB480FF, 0x005B8DFF], // 161
    &[0xD46D7AFF, 0x8FA071FF, 0xB984AFFF], // 162
    &[0xFFDD00FF, 0xB5DECCFF, 0x007190FF], // 163
    &[0xDD4027FF, 0xFCB315FF, 0x4F4086FF], // 164
    &[0xE0B3B6FF, 0xF27291FF, 0x6D4145FF], // 165
    &[0xF48067FF, 0xFBE6A0FF, 0x112F2CFF], // 166
    &[0xC2AE93FF, 0xA7D4E4FF, 0x064F6EFF], // 167
    &[0xF8ED43FF, 0x064F6EFF, 0x8C4C62FF], // 168
    &[0xF8B6BAFF, 0xFFEFAEFF, 0xA1A39AFF], // 169
    &[0xB73F74FF, 0xFCB315FF, 0x59256AFF], // 170
    &[0x802626FF, 0xF99D1BFF, 0xB4CDC2FF], // 171
    &[0xC27544FF, 0x007190FF, 0x59256AFF], // 172
    &[0xF8ED43FF, 0x762C19FF, 0xB5DECCFF], // 173
    &[0xF8B6BAFF, 0xC0A9B3FF, 0x7A4456FF], // 174
    &[0xEEB480FF, 0xC1C494FF, 0x6450A1FF], // 175
    &[0xF9C1CEFF, 0xFDD4BDFF, 0x78CDD0FF], // 176
    &[0x802626FF, 0x96874DFF, 0xB5B1D8FF], // 177
    &[0xEBD3A2FF, 0xA5C8D1FF, 0x099197FF], // 178
    &[0xDD4027FF, 0xF3A257FF, 0x1C4286FF], // 179
    &[0xFDC57EFF, 0x9A72AAFF, 0xB6BFC1FF], // 180
    &[0xA62C37FF, 0x4F4086FF, 0x501345FF], // 181
    &[0xBB7125FF, 0x4B3317FF, 0x051230FF], // 182
    &[0x70727CFF, 0x8C4C62FF, 0x59256AFF], // 183
    &[0xF27291FF, 0xEBD3A2FF, 0x848061FF], // 184
    &[0xB59392FF, 0xC55347FF, 0xFFEFAEFF], // 185
    &[0x793327FF, 0xD8A37BFF, 0x006EB8FF], // 186
    &[0x005B8DFF, 0xC0A9B3FF, 0xA36AA5FF], // 187
    &[0x8FA071FF, 0x97ACC8FF, 0x96D1AAFF], // 188
    &[0xF8ED43FF, 0x253122FF, 0x62C6BFFF], // 189
    &[0xEBD3A2FF, 0xD96629FF, 0x111314FF], // 190
    &[0xB59392FF, 0xE2B540FF, 0x006EB8FF], // 191
    &[0xFDBF68FF, 0x4B3317FF, 0x70727CFF], // 192
    &[0xF48067FF, 0xFBE6A0FF, 0x00908AFF], // 193
    &[0xEB5324FF, 0xFDD4BDFF, 0x5A82B3FF], // 194
    &[0xF27291FF, 0xFFEFAEFF, 0xB6BFC1FF], // 195
    &[0xB2B73EFF, 0xA7D4E4FF, 0x6450A1FF], // 196
    &[0xF37F94FF, 0x66629CFF, 0xB6BFC1FF], // 197
    &[0xAE5224FF, 0xFFDD00FF, 0x489B6EFF], // 198
    &[0xAB544DFF, 0x806E4BFF, 0x1C4286FF], // 199
    &[0xA62C37FF, 0xC1C494FF, 0x719470FF], // 200
    &[0xF48067FF, 0x837E31FF, 0x96D1AAFF], // 201
    &[0xB5DECCFF, 0x96D1AAFF, 0x34454CFF], // 202
    &[0xFFEFAEFF, 0xEEA78CFF, 0x555832FF], // 203
    &[0xB73F74FF, 0xC27544FF, 0xA5C8D1FF], // 204
    &[0x802626FF, 0xEEA78CFF, 0x4F4086FF], // 205
    &[0xF8B6BAFF, 0xF3A257FF, 0xC27544FF], // 206
    &[0xA36752FF, 0xB4CDC2FF, 0x111314FF], // 207
    &[0xF5ECC2FF, 0x099197FF, 0x007190FF], // 208
    &[0xEBD3A2FF, 0xF99D1BFF, 0x97ACC8FF], // 209
    &[0xFDC57EFF, 0xF8ED43FF, 0x555832FF], // 210
    &[0xF68C50FF, 0xA6A159FF, 0x051230FF], // 211
    &[0xAB2439FF, 0x986F2DFF, 0x97ACC8FF], // 212
    &[0xEEA78CFF, 0xFFDD00FF, 0xA7D4E4FF], // 213
    &[0xEBD3A2FF, 0xA36752FF, 0x4F4086FF], // 214
    &[0xFDBF68FF, 0x006EB8FF, 0xBF5892FF], // 215
    &[0xEB5324FF, 0x489B6EFF, 0x111314FF], // 216
    &[0x802626FF, 0xD8A37BFF, 0x1A7444FF], // 217
    &[0x005B8DFF, 0xB5B1D8FF, 0x70727CFF], // 218
    &[0xEB5324FF, 0x719470FF, 0x004F46FF], // 219
    &[0xB71F57FF, 0xD8A37BFF, 0xA36AA5FF], // 220
    &[0xA62C37FF, 0xB6BFC1FF, 0x111314FF], // 221
    &[0xE2B540FF, 0xF99D1BFF, 0xC16B27FF], // 222
    &[0xB59392FF, 0xD8A37BFF, 0xB5DECCFF], // 223
    &[0xF27291FF, 0x547076FF, 0x7A4456FF], // 224
    &[0xCC1236FF, 0x004F46FF, 0x704357FF], // 225
    &[0x6D4145FF, 0xFDBF68FF, 0x4F4086FF], // 226
    &[0xF9C1CEFF, 0xA5C8D1FF, 0x0093A5FF], // 227
    &[0xA62C37FF, 0xFFEFAEFF, 0xB6BFC1FF], // 228
    &[0xF3A257FF, 0x253122FF, 0xB6BFC1FF], // 229
    &[0xF48067FF, 0xB5DECCFF, 0x96D1AAFF], // 230
    &[0xE0B3B6FF, 0x793327FF, 0x5A82B3FF], // 231
    &[0xCC1236FF, 0xEEB480FF, 0x051230FF], // 232
    &[0xA62C37FF, 0x96874DFF, 0x40456AFF], // 233
    &[0xFDC57EFF, 0x71502FFF, 0xA7D4E4FF], // 234
    &[0xEBD3A2FF, 0xF99D1BFF, 0xC0A9B3FF], // 235
    &[0xBC892BFF, 0x1C4286FF, 0x84565BFF], // 236
    &[0xA62C37FF, 0x762C19FF, 0x97ACC8FF], // 237
    &[0xD8A37BFF, 0x501345FF, 0xA1A39AFF], // 238
    &[0xB59392FF, 0xCAB356FF, 0xB4CDC2FF], // 239
    &[0xF6917EFF, 0xFFF200FF, 0x0093A5FF], // 240
    &[0xDD4027FF, 0xFFEFAEFF, 0xC5A56EFF, 0x547076FF], // 241
    &[0xF37F94FF, 0xAE5224FF, 0x1A7444FF, 0x111314FF], // 242
    &[0xBB7125FF, 0xEBD3A2FF, 0x6B7140FF, 0x34454CFF], // 243
    &[0xB59392FF, 0xC56127FF, 0x6D7E77FF, 0x007190FF], // 244
    &[0xA62C37FF, 0x819238FF, 0x12354EFF, 0x34454CFF], // 245
    &[0xF8B6BAFF, 0xA84222FF, 0xF5ECC2FF, 0xFDC57EFF], // 246
    &[0xBB7125FF, 0xFFDD00FF, 0x00978DFF, 0x1C4286FF], // 247
    &[0xF37F94FF, 0xBC892BFF, 0xB5B1D8FF, 0x704357FF], // 248
    &[0x793327FF, 0xC2AE93FF, 0xD6B43EFF, 0x547076FF], // 249
    &[0xCAB356FF, 0xF15A30FF, 0x00B49BFF, 0xBCE4E5FF], // 250
    &[0xA72144FF, 0xFFF200FF, 0x1A7444FF, 0x34454CFF], // 251
    &[0xDA525DFF, 0xBB7125FF, 0xC19F2CFF, 0x099197FF], // 252
    &[0xF8ED43FF, 0xF68C50FF, 0x501345FF, 0x34454CFF], // 253
    &[0xF8B6BAFF, 0xF5ECC2FF, 0x837E31FF, 0xCA92A8FF], // 254
    &[0xBB7125FF, 0xCAB356FF, 0x78CDD0FF, 0x111314FF], // 255
    &[0xEEA78CFF, 0xF37420FF, 0x009465FF, 0x111314FF], // 256
    &[0xE31F26FF, 0xFCB315FF, 0x006EB8FF, 0xA36AA5FF], // 257
    &[0x802626FF, 0xEEB480FF, 0x837E31FF, 0x007190FF], // 258
    &[0xF8ED43FF, 0x099197FF, 0x005B8DFF, 0xA1A39AFF], // 259
    &[0xD46D7AFF, 0xEEA78CFF, 0xB4CDC2FF, 0x00B49BFF], // 260
    &[0xA72144FF, 0xFFEFAEFF, 0x78CDD0FF, 0xA1A39AFF], // 261
    &[0xDA525DFF, 0xEBD3A2FF, 0xB09F36FF, 0x437742FF], // 262
    &[0xAE5224FF, 0xEEB480FF, 0xB5DECCFF, 0x34454CFF], // 263
    &[0xF8B6BAFF, 0xDD4027FF, 0xB7C2A9FF, 0x0093A5FF], // 264
    &[0xD46D7AFF, 0xFFDD00FF, 0xA6A159FF, 0x1E0E3FFF], // 265
    &[0xE31F26FF, 0xEBD3A2FF, 0x8FA071FF, 0x00978DFF], // 266
    &[0xFDBF68FF, 0xF99D1BFF, 0x00978DFF, 0x006EB8FF], // 267
    &[0xB59392FF, 0xBB7125FF, 0x253122FF, 0xBCE4E5FF], // 268
    &[0xBB7125FF, 0x802626FF, 0xA36AA5FF, 0x111314FF], // 269
    &[0xDA525DFF, 0xF5ECC2FF, 0x6B7140FF, 0x437742FF], // 270
    &[0xB71F57FF, 0x96D1AAFF, 0x099197FF, 0x112F2CFF], // 271
    &[0xFFEFAEFF, 0xF37420FF, 0xB5DECCFF, 0x97ACC8FF], // 272
    &[0xF9C1CEFF, 0x7D133AFF, 0xA36752FF, 0xB6BFC1FF], // 273
    &[0xF15A30FF, 0x8B835BFF, 0x5A82B3FF, 0x9A72AAFF], // 274
    &[0xC55347FF, 0xC2AE93FF, 0x762C19FF, 0x7A4456FF], // 275
    &[0xF37F94FF, 0xFDD4BDFF, 0xAFD472FF, 0x111314FF], // 276
    &[0xF27291FF, 0xB73F74FF, 0x837E31FF, 0x1E0E3FFF], // 277
    &[0xFDBF68FF, 0xD6B43EFF, 0x437742FF, 0x004F46FF], // 278
    &[0xBB7125FF, 0xEEA78CFF, 0xC2AE93FF, 0x12354EFF], // 279
    &[0xDA525DFF, 0x555832FF, 0xCA92A8FF, 0x7A4456FF], // 280
    &[0xFFEFAEFF, 0x00978DFF, 0x96D1AAFF, 0x007190FF], // 281
    &[0xDA525DFF, 0xC59F6BFF, 0x96D1AAFF, 0xB984AFFF], // 282
    &[0xAB544DFF, 0x802626FF, 0x719470FF, 0x62C6BFFF], // 283
    &[0xE2625EFF, 0xFFDD00FF, 0x00B49BFF, 0x004F46FF], // 284
    &[0xB59392FF, 0xAE5224FF, 0xF15A30FF, 0xB5DECCFF], // 285
    &[0xAE5224FF, 0xFCB315FF, 0x00939BFF, 0x40456AFF], // 286
    &[0xF37F94FF, 0xCAB356FF, 0xA7D4E4FF, 0x78CDD0FF], // 287
    &[0xF99D1BFF, 0x644B1EFF, 0x7A4456FF, 0x111314FF], // 288
    &[0xF8ED43FF, 0xC7D14FFF, 0x40456AFF, 0x1E0E3FFF], // 289
    &[0x6D4145FF, 0xFFEFAEFF, 0x555832FF, 0x96D1AAFF], // 290
    &[0xC7D14FFF, 0x00B49BFF, 0x96D1AAFF, 0x78CDD0FF], // 291
    &[0xFFEFAEFF, 0xEEB480FF, 0xC5A56EFF, 0xC2AE93FF], // 292
    &[0xBB7125FF, 0xB5DECCFF, 0x709390FF, 0x489B6EFF], // 293
    &[0xF5ECC2FF, 0xFDBF68FF, 0x437742FF, 0x97ACC8FF], // 294
    &[0xFDBF68FF, 0xFFF200FF, 0x006EB8FF, 0x1E0E3FFF], // 295
    &[0xF5ECC2FF, 0xD8A37BFF, 0x71502FFF, 0x34454CFF], // 296
    &[0xAE5224FF, 0xF99D1BFF, 0x6B7140FF, 0x40456AFF], // 297
    &[0xBB7125FF, 0xF8ED43FF, 0xF15A30FF, 0x111314FF], // 298
    &[0xC53C69FF, 0xEEA78CFF, 0x819238FF, 0x007190FF], // 299
    &[0xF48067FF, 0xFDBF68FF, 0xB5DECCFF, 0x78CDD0FF], // 300
    &[0xE31F26FF, 0xEBD3A2FF, 0x8FA071FF, 0xA36AA5FF], // 301
    &[0xFDBF68FF, 0xC2AE93FF, 0xBCE4E5FF, 0x007190FF], // 302
    &[0xFBE6A0FF, 0xF15A30FF, 0x253122FF, 0xB6BFC1FF], // 303
    &[0x793327FF, 0xFDBF68FF, 0x8B835BFF, 0x00978DFF], // 304
    &[0xEEB480FF, 0xFFDD00FF, 0xB2B73EFF, 0xB5DECCFF], // 305
    &[0xF8ED43FF, 0x00978DFF, 0x009465FF, 0xBCE4E5FF], // 306
    &[0xCC1236FF, 0xB5B1D8FF, 0xA36AA5FF, 0x501345FF], // 307
    &[0xD1B0A7FF, 0xCB2F43FF, 0xD96629FF, 0x96D1AAFF], // 308
    &[0xF3A257FF, 0xF68C50FF, 0x40456AFF, 0x064F6EFF], // 309
    &[0xF5ECC2FF, 0xEEB480FF, 0x837E31FF, 0x253122FF], // 310
    &[0xAB2439FF, 0xFDBF68FF, 0xB7C2A9FF, 0xC7D14FFF], // 311
    &[0xAE5224FF, 0xF99D1BFF, 0x709390FF, 0x005B8DFF], // 312
    &[0xCC1236FF, 0xFFF200FF, 0x1A7444FF, 0x111314FF], // 313
    &[0xF37F94FF, 0x793327FF, 0x1C4286FF, 0x4E1D4CFF], // 314
    &[0xF48067FF, 0xF5ECC2FF, 0xF3A257FF, 0xBF5892FF], // 315
    &[0x82241FFF, 0x009465FF, 0x4F4086FF, 0x59256AFF], // 316
    &[0xFCC79BFF, 0xC2AE93FF, 0xF8ED43FF, 0xB5DECCFF], // 317
    &[0x806E4BFF, 0x42533EFF, 0x004F46FF, 0x112F2CFF], // 318
    &[0xBB7125FF, 0xFFDD00FF, 0xF99D1BFF, 0x437742FF], // 319
    &[0xF58E84FF, 0xF5ECC2FF, 0x819238FF, 0xA5C8D1FF], // 320
    &[0xB59392FF, 0xF5ECC2FF, 0x253122FF, 0x97ACC8FF], // 321
    &[0xE31F26FF, 0xA84222FF, 0xBF5892FF, 0x6450A1FF], // 322
    &[0xFDC57EFF, 0xB2B73EFF, 0x762C19FF, 0x111314FF], // 323
    &[0xAB2439FF, 0x5A82B3FF, 0xA36AA5FF, 0xB6BFC1FF], // 324
    &[0xDA525DFF, 0xFBE6A0FF, 0xE2B540FF, 0x112F2CFF], // 325
    &[0xF5ECC2FF, 0xF15A30FF, 0xAFD472FF, 0x87C540FF], // 326
    &[0xF37F94FF, 0xBB7125FF, 0xC2AE93FF, 0xC0A9B3FF], // 327
    &[0xA84222FF, 0xF68C50FF, 0x4B3317FF, 0x00908AFF], // 328
    &[0xFDBF68FF, 0xC0A9B3FF, 0x501345FF, 0x34454CFF], // 329
    &[0xC1C494FF, 0xBCE4E5FF, 0x97ACC8FF, 0x099197FF], // 330
    &[0xC53C69FF, 0x1E0E3FFF, 0x9A72AAFF, 0x4F4086FF], // 331
    &[0xF58E84FF, 0xCB2F43FF, 0x253122FF, 0x004F46FF], // 332
    &[0xAE5224FF, 0xF8ED43FF, 0x96D1AAFF, 0x006EB8FF], // 333
    &[0xFDD4BDFF, 0x837E31FF, 0xAFD472FF, 0x007190FF], // 334
    &[0x82241FFF, 0xF99D1BFF, 0x4F4086FF, 0x34454CFF], // 335
    &[0xF37F94FF, 0x793327FF, 0xFFEFAEFF, 0x42533EFF], // 336
    &[0x6D4145FF, 0xCA92A8FF, 0x713B4CFF, 0x111314FF], // 337
    &[0xA62C37FF, 0xFCB315FF, 0x004F46FF, 0xC0A9B3FF], // 338
    &[0xD8A37BFF, 0xD96629FF, 0xA5C8D1FF, 0x40456AFF], // 339
    &[0xF15A30FF, 0x00B49BFF, 0xB6BFC1FF, 0x111314FF], // 340
    &[0xF48067FF, 0x437742FF, 0x253122FF, 0xA5C8D1FF], // 341
    &[0xF8B6BAFF, 0xFDBF68FF, 0x986F2DFF, 0x253122FF], // 342
    &[0xAE5224FF, 0xEBD3A2FF, 0x635A3AFF, 0x064F6EFF], // 343
    &[0xFDC57EFF, 0x1C4286FF, 0xA36AA5FF, 0x111314FF], // 344
    &[0x793327FF, 0xBCE4E5FF, 0x62C6BFFF, 0x6450A1FF], // 345
    &[0xB73F74FF, 0xB5DECCFF, 0xC7D14FFF, 0x6D7E77FF], // 346
    &[0xA6A159FF, 0x00B49BFF, 0x005B8DFF, 0xB984AFFF], // 347
    &[0xC1C494FF, 0x437742FF, 0x253122FF, 0x501345FF], // 348
];

/// The same book's two-colour combinations, numbers 1 to 120, from the same
/// data set and under the same notice as the table above, and in the same
/// form: the book's order, its numbers in the comments.
///
/// Apart from the others because they answer a different question. A pair is
/// half of a four-colour theme, which is why the table above leaves them
/// out; it is the whole of a two-colour one, an accent and the page it
/// stands on, and that is what the builder offers them for.
pub const PAIRS: &[&[u32]] = &[
    &[0xD96629FF, 0x0093A5FF], // 1
    &[0xF99D1BFF, 0x12354EFF], // 2
    &[0xBB7125FF, 0xFFEFAEFF], // 3
    &[0xC5A56EFF, 0x59256AFF], // 4
    &[0x437742FF, 0x064F6EFF], // 5
    &[0xF48067FF, 0x051230FF], // 6
    &[0xF37420FF, 0xB4CDC2FF], // 7
    &[0xC27544FF, 0xB5B1D8FF], // 8
    &[0x642D5EFF, 0x80719EFF], // 9
    &[0xC27544FF, 0x8B835BFF], // 10
    &[0xEBD3A2FF, 0xA2B0ADFF], // 11
    &[0xC5A56EFF, 0x099197FF], // 12
    &[0xBB7125FF, 0x8C4C62FF], // 13
    &[0xF27291FF, 0xFBE6A0FF], // 14
    &[0x00978DFF, 0xB5B1D8FF], // 15
    &[0x82241FFF, 0xA7D4E4FF], // 16
    &[0xDA525DFF, 0x00B49BFF], // 17
    &[0xD1B0A7FF, 0x4E1D4CFF], // 18
    &[0x653514FF, 0x87C540FF], // 19
    &[0x78CDD0FF, 0xCA92A8FF], // 20
    &[0xF48067FF, 0x00B49BFF], // 21
    &[0xFFF200FF, 0x1C4286FF], // 22
    &[0xFDC57EFF, 0x9A72AAFF], // 23
    &[0x644B1EFF, 0x8C4C62FF], // 24
    &[0xC55347FF, 0xBCE4E5FF], // 25
    &[0xF3A257FF, 0x71502FFF], // 26
    &[0xF8B6BAFF, 0x34454CFF], // 27
    &[0x762C19FF, 0x051230FF], // 28
    &[0x84875EFF, 0x97ACC8FF], // 29
    &[0xAB2439FF, 0xA2B0ADFF], // 30
    &[0xDD4027FF, 0xFFEFAEFF], // 31
    &[0xD8A37BFF, 0x87C540FF], // 32
    &[0xBB7125FF, 0x34454CFF], // 33
    &[0xF37F94FF, 0xB6BFC1FF], // 34
    &[0xB59392FF, 0xA62C37FF], // 35
    &[0xC19F2CFF, 0xB5DECCFF], // 36
    &[0xA84222FF, 0x59256AFF], // 37
    &[0x1A7444FF, 0x1C4286FF], // 38
    &[0xCC1236FF, 0x005B8DFF], // 39
    &[0xC56127FF, 0xB2B73EFF], // 40
    &[0x8B835BFF, 0x78CDD0FF], // 41
    &[0xE2B540FF, 0x4F4086FF], // 42
    &[0xF8B6BAFF, 0xA36AA5FF], // 43
    &[0x00908AFF, 0x5A82B3FF], // 44
    &[0xFDD4BDFF, 0xF8ED43FF], // 45
    &[0xF37420FF, 0x111314FF], // 46
    &[0xC55347FF, 0xC0A9B3FF], // 47
    &[0xB73F74FF, 0x005B8DFF], // 48
    &[0xA7D4E4FF, 0x006EB8FF], // 49
    &[0xEBD3A2FF, 0x4E1D4CFF], // 50
    &[0xA62C37FF, 0x006EB8FF], // 51
    &[0xF5ECC2FF, 0x111314FF], // 52
    &[0xF99D1BFF, 0x4E1D4CFF], // 53
    &[0x00978DFF, 0xA5C8D1FF], // 54
    &[0xD46D7AFF, 0xFFFFFFFF], // 55
    &[0xC0A9B3FF, 0x4F4086FF], // 56
    &[0x7A4456FF, 0x34454CFF], // 57
    &[0x793327FF, 0x00B49BFF], // 58
    &[0xF37F94FF, 0xB09F36FF], // 59
    &[0xFFEFAEFF, 0x12354EFF], // 60
    &[0xC7D14FFF, 0x501345FF], // 61
    &[0xFFF200FF, 0x111314FF], // 62
    &[0x6D4145FF, 0x0093A5FF], // 63
    &[0xA36AA5FF, 0x66629CFF], // 64
    &[0xC19F2CFF, 0x78CDD0FF], // 65
    &[0xD6B43EFF, 0x6B7140FF], // 66
    &[0x5A82B3FF, 0x12354EFF], // 67
    &[0xB59392FF, 0xFFF200FF], // 68
    &[0xA1A39AFF, 0x111314FF], // 69
    &[0xBB7125FF, 0x555832FF], // 70
    &[0xAB2439FF, 0xD8A37BFF], // 71
    &[0xF5ECC2FF, 0xA7D4E4FF], // 72
    &[0x71502FFF, 0x8FA071FF], // 73
    &[0xB5DECCFF, 0x099197FF], // 74
    &[0xA7D4E4FF, 0x40456AFF], // 75
    &[0xFFEFAEFF, 0xA1A39AFF], // 76
    &[0xDA525DFF, 0x064F6EFF], // 77
    &[0xEEB480FF, 0x62C6BFFF], // 78
    &[0x762C19FF, 0x099197FF], // 79
    &[0xF5ECC2FF, 0x9A72AAFF], // 80
    &[0xF3A257FF, 0xA1A39AFF], // 81
    &[0x793327FF, 0x4E1D4CFF], // 82
    &[0xC1C494FF, 0x40456AFF], // 83
    &[0xFDD4BDFF, 0x112F2CFF], // 84
    &[0xC56127FF, 0x007190FF], // 85
    &[0xBB7125FF, 0x00B49BFF], // 86
    &[0xF8B6BAFF, 0xB2B73EFF], // 87
    &[0xFDD4BDFF, 0x006EB8FF], // 88
    &[0xF99D1BFF, 0x40456AFF], // 89
    &[0xF37F94FF, 0xA36AA5FF], // 90
    &[0x6D4145FF, 0xC16B27FF], // 91
    &[0xF58E84FF, 0x00978DFF], // 92
    &[0xB09F36FF, 0xA5C8D1FF], // 93
    &[0xEBD3A2FF, 0x004F46FF], // 94
    &[0x793327FF, 0x1E0E3FFF], // 95
    &[0xE2B540FF, 0x837E31FF], // 96
    &[0xF8B6BAFF, 0xC55347FF], // 97
    &[0x762C19FF, 0x40456AFF], // 98
    &[0xFFEFAEFF, 0x0093A5FF], // 99
    &[0x96874DFF, 0x80719EFF], // 100
    &[0xE0B3B6FF, 0x1C4286FF], // 101
    &[0xEBD3A2FF, 0xC16B27FF], // 102
    &[0xC27544FF, 0x4E1D4CFF], // 103
    &[0xA62C37FF, 0xF5ECC2FF], // 104
    &[0xE0B3B6FF, 0x719470FF], // 105
    &[0x007190FF, 0x1E0E3FFF], // 106
    &[0xFFDD00FF, 0x848061FF], // 107
    &[0xF37F94FF, 0xA84222FF], // 108
    &[0xFFEFAEFF, 0x42533EFF], // 109
    &[0x7C4226FF, 0x4B3317FF], // 110
    &[0xFFEFAEFF, 0xAFD472FF], // 111
    &[0xF48067FF, 0x111314FF], // 112
    &[0xFDD4BDFF, 0x4B3317FF], // 113
    &[0xFCB315FF, 0x007190FF], // 114
    &[0xFBE6A0FF, 0xF15A30FF], // 115
    &[0xE0B3B6FF, 0x6450A1FF], // 116
    &[0xCC1236FF, 0x111314FF], // 117
    &[0xE2B540FF, 0x4B3317FF], // 118
    &[0xA5C8D1FF, 0x12354EFF], // 119
    &[0xE0B3B6FF, 0xAB2439FF], // 120
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The table is what its header says: the threes and the fours and
    /// nothing else, every colour opaque, and no combination twice.
    #[test]
    fn the_table_is_the_threes_and_the_fours() {
        assert_eq!(COMBINATIONS.len(), 228);
        assert_eq!(COMBINATIONS.iter().filter(|row| row.len() == 3).count(), 120);
        assert_eq!(COMBINATIONS.iter().filter(|row| row.len() == 4).count(), 108);
        for (at, row) in COMBINATIONS.iter().enumerate() {
            assert!(row.iter().all(|rgba| rgba & 0xFF == 0xFF), "row {at} has a colour that is not opaque");
            assert!(!COMBINATIONS[..at].contains(row), "row {at} is in the table twice");
        }
    }

    /// The threes come first, as they do in the book, so a row's place in
    /// the table is its number less a hundred and twenty-one.
    #[test]
    fn the_rows_are_in_the_books_order() {
        assert!(COMBINATIONS[..120].iter().all(|row| row.len() == 3));
        assert!(COMBINATIONS[120..].iter().all(|row| row.len() == 4));
    }

    /// The pairs are the book's first hundred and twenty, two colours each,
    /// every colour opaque, and none of them a row of the table above.
    #[test]
    fn the_pairs_are_the_books_first_hundred_and_twenty() {
        assert_eq!(PAIRS.len(), 120);
        for (at, row) in PAIRS.iter().enumerate() {
            assert_eq!(row.len(), 2, "pair {} is not two colours", at + 1);
            assert!(row.iter().all(|rgba| rgba & 0xFF == 0xFF), "pair {} has a colour that is not opaque", at + 1);
            assert!(!PAIRS[..at].contains(row), "pair {} is in the table twice", at + 1);
        }
    }
}
