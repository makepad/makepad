cp chinese_bold/resources/LXGWWenKaiBold.ttf .

rm xa*
split -b 10000000 LXGWWenKaiBold.ttf
cp xaa chinese_bold/resources/LXGWWenKaiBold.ttf
cp xab chinese_bold_2/resources/LXGWWenKaiBold.ttf.2

rm xa*
cp chinese_regular/resources/LXGWWenKaiRegular.ttf .
split -b 10000000 LXGWWenKaiRegular.ttf
cp xaa chinese_regular/resources/LXGWWenKaiRegular.ttf
cp xab chinese_regular_2/resources/LXGWWenKaiRegular.ttf.2
rm xa*

