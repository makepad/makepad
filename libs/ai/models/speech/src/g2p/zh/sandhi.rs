//! Mandarin tone sandhi, erhua and the word merges that prepare them: a port of
//! misaki's `tone_sandhi.py` (Apache-2.0, itself adapted from PaddleSpeech),
//! including its word lists. `finals` are tone-digit-last strings (`ang1`).

use super::{pinyin, segment};
use std::collections::HashSet;
use std::sync::OnceLock;

const BU: char = '不';
const YI: char = '一';
const PUNC: &str = "、：，；。？！“”‘’':,;.?!";

const MUST_NEUTRAL: &str = "麻烦 麻利 鸳鸯 高粱 骨头 骆驼 马虎 首饰 馒头 馄饨 风筝 难为 队伍 阔气 闺女 门道 锄头 铺盖 铃铛 铁匠 钥匙 里脊 里头 部分 那么 道士 造化 迷糊 连累 这么 这个 运气 过去 软和 转悠 踏实 跳蚤 跟头 趔趄 财主 豆腐 讲究 记性 记号 认识 规矩 见识 裁缝 补丁 衣裳 衣服 衙门 街坊 行李 行当 蛤蟆 蘑菇 薄荷 葫芦 葡萄 萝卜 荸荠 苗条 苗头 苍蝇 芝麻 舒服 舒坦 舌头 自在 膏药 脾气 脑袋 脊梁 能耐 胳膊 胭脂 胡萝 胡琴 胡同 聪明 耽误 耽搁 耷拉 耳朵 老爷 老实 老婆 戏弄 将军 翻腾 罗嗦 罐头 编辑 结实 红火 累赘 糨糊 糊涂 精神 粮食 簸箕 篱笆 算计 算盘 答应 笤帚 笑语 笑话 窟窿 窝囊 窗户 稳当 稀罕 称呼 秧歌 秀气 秀才 福气 祖宗 砚台 码头 石榴 石头 石匠 知识 眼睛 眯缝 眨巴 眉毛 相声 盘算 白净 痢疾 痛快 疟疾 疙瘩 疏忽 畜生 生意 甘蔗 琵琶 琢磨 琉璃 玻璃 玫瑰 玄乎 狐狸 状元 特务 牲口 牙碜 牌楼 爽快 爱人 热闹 烧饼 烟筒 烂糊 点心 炊帚 灯笼 火候 漂亮 滑溜 溜达 温和 清楚 消息 浪头 活泼 比方 正经 欺负 模糊 槟榔 棺材 棒槌 棉花 核桃 栅栏 柴火 架势 枕头 枇杷 机灵 本事 木头 木匠 朋友 月饼 月亮 暖和 明白 时候 新鲜 故事 收拾 收成 提防 挖苦 挑剔 指甲 指头 拾掇 拳头 拨弄 招牌 招呼 抬举 护士 折腾 扫帚 打量 打算 打扮 打听 打发 扎实 扁担 戒指 懒得 意识 意思 悟性 怪物 思量 怎么 念头 念叨 别人 快活 忙活 志气 心思 得罪 张罗 弟兄 开通 应酬 庄稼 干事 帮手 帐篷 希罕 师父 师傅 巴结 巴掌 差事 工夫 岁数 屁股 尾巴 少爷 小气 小伙 将就 对头 对付 寡妇 家伙 客气 实在 官司 学问 字号 嫁妆 媳妇 媒人 婆家 娘家 委屈 姑娘 姐夫 妯娌 妥当 妖精 奴才 女婿 头发 太阳 大爷 大方 大意 大夫 多少 多么 外甥 壮实 地道 地方 在乎 困难 嘴巴 嘱咐 嘟囔 嘀咕 喜欢 喇嘛 喇叭 商量 唾沫 哑巴 哈欠 哆嗦 咳嗽 和尚 告诉 告示 含糊 吓唬 后头 名字 名堂 合同 吆喝 叫唤 口袋 厚道 厉害 千斤 包袱 包涵 匀称 勤快 动静 动弹 功夫 力气 前头 刺猬 刺激 别扭 利落 利索 利害 分析 出息 凑合 凉快 冷战 冤枉 冒失 养活 关系 先生 兄弟 便宜 使唤 佩服 作坊 体面 位置 似的 伙计 休息 什么 人家 亲戚 亲家 交情 云彩 事情 买卖 主意 丫头 丧气 两口 东西 东家 世故 不由 下水 下巴 上头 上司 丈夫 丈人 一辈 那个 菩萨 父亲 母亲 咕噜 邋遢 费用 冤家 甜头 介绍 荒唐 大人 泥鳅 幸福 熟悉 计划 扑腾 蜡烛 姥爷 照顾 喉咙 吉他 弄堂 蚂蚱 凤凰 拖沓 寒碜 糟蹋 倒腾 报复 逻辑 盘缠 喽啰 牢骚 咖喱 扫把 惦记";
const MUST_NOT_NEUTRAL: &str = "男子 女子 分子 原子 量子 莲子 石子 瓜子 电子 人人 虎虎 幺幺 干嘛 学子 哈哈 数数 袅袅 局地 以下 娃哈哈 花花草草 留得 耕地 想想 熙熙 攘攘 卵子 死死 冉冉 恳恳 佼佼 吵吵 打打 考考 整整 莘莘 落地 算子 家家户户 青青";
pub const MUST_ERHUA: &str = "小院儿 胡同儿 范儿 老汉儿 撒欢儿 寻老礼儿 妥妥儿 媳妇儿";
pub const NOT_ERHUA: &str = "虐儿 为儿 护儿 瞒儿 救儿 替儿 有儿 一儿 我儿 俺儿 妻儿 拐儿 聋儿 乞儿 患儿 幼儿 孤儿 婴儿 婴幼儿 连体儿 脑瘫儿 流浪儿 体弱儿 混血儿 蜜雪儿 舫儿 祖儿 美儿 应采儿 可儿 侄儿 孙儿 侄孙儿 女儿 男儿 红孩儿 花儿 虫儿 马儿 鸟儿 猪儿 猫儿 狗儿 少儿";

fn set(words: &'static str) -> HashSet<&'static str> {
    words.split(' ').collect()
}

fn must_neutral() -> &'static HashSet<&'static str> {
    static S: OnceLock<HashSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| set(MUST_NEUTRAL))
}

fn must_not_neutral() -> &'static HashSet<&'static str> {
    static S: OnceLock<HashSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| set(MUST_NOT_NEUTRAL))
}

pub fn must_erhua() -> &'static HashSet<&'static str> {
    static S: OnceLock<HashSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| set(MUST_ERHUA))
}

pub fn not_erhua() -> &'static HashSet<&'static str> {
    static S: OnceLock<HashSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| set(NOT_ERHUA))
}

fn is_x_eng(pos: &str) -> bool {
    pos == "x" || pos == "eng"
}

/// Replace the tone digit (last char) of a final.
fn with_tone(final_: &str, tone: char) -> String {
    let mut s = final_.to_string();
    s.pop();
    s.push(tone);
    s
}

fn tone_of(final_: &str) -> char {
    final_.chars().last().unwrap_or('0')
}

fn all_tone_three(finals: &[String]) -> bool {
    finals.iter().all(|f| tone_of(f) == '3')
}

fn tail2(word: &str) -> String {
    let chars: Vec<char> = word.chars().collect();
    chars[chars.len().saturating_sub(2)..].iter().collect()
}

/// jieba's `cut_for_search` then misaki's pick: the shortest piece splits the word in two.
fn split_word(word: &str) -> (String, String) {
    let mut pieces = segment::cut_for_search(word);
    pieces.sort_by_key(|p| p.chars().count());
    let first = pieces.first().cloned().unwrap_or_else(|| word.to_string());
    let chars: Vec<char> = word.chars().collect();
    let first_len = first.chars().count();
    let begin = word.find(&first).map(|b| word[..b].chars().count()).unwrap_or(0);
    if begin == 0 {
        (first, chars[first_len.min(chars.len())..].iter().collect())
    } else {
        (chars[..chars.len().saturating_sub(first_len)].iter().collect(), first)
    }
}

fn neutral_sandhi(word: &str, pos: &str, finals: &mut Vec<String>) {
    if must_not_neutral().contains(word) {
        return;
    }
    let chars: Vec<char> = word.chars().collect();
    let n = chars.len();
    // Reduplication of nouns, verbs and adjectives: 奶奶, 试试.
    if pos.starts_with(['n', 'v', 'a']) {
        for j in 1..n {
            if chars[j] == chars[j - 1] {
                finals[j] = with_tone(&finals[j], '5');
            }
        }
    }
    let last = chars[n - 1];
    let ge_idx = chars.iter().position(|c| *c == '个');
    let mut applied = true;
    if "吧呢啊呐噻嘛吖嗨呐哦哒滴哩哟喽啰耶喔诶".contains(last) {
        finals[n - 1] = with_tone(&finals[n - 1], '5');
    } else if "的地得".contains(last) {
        finals[n - 1] = with_tone(&finals[n - 1], '5');
    } else if n == 1 && "了着过".contains(word) && matches!(pos, "ul" | "uz" | "ug") {
        finals[n - 1] = with_tone(&finals[n - 1], '5');
    } else if n > 1 && "们子".contains(last) && matches!(pos, "r" | "n") {
        finals[n - 1] = with_tone(&finals[n - 1], '5');
    } else if n > 1 && "上下".contains(last) && matches!(pos, "s" | "l" | "f") {
        finals[n - 1] = with_tone(&finals[n - 1], '5');
    } else if n > 1 && "来去".contains(last) && "上下进出回过起开".contains(chars[n - 2]) {
        finals[n - 1] = with_tone(&finals[n - 1], '5');
    } else if let Some(g) = ge_idx.filter(|g| {
        *g >= 1 && (chars[*g - 1].is_numeric() || "几有两半多各整每做是".contains(chars[*g - 1]))
    }) {
        finals[g] = with_tone(&finals[g], '5');
    } else if word == "个" {
        finals[0] = with_tone(&finals[0], '5');
    } else {
        applied = false;
        if must_neutral().contains(word) || must_neutral().contains(tail2(word).as_str()) {
            finals[n - 1] = with_tone(&finals[n - 1], '5');
        }
    }
    let _ = applied;
    let (first, second) = split_word(word);
    let first_len = first.chars().count();
    for (index, piece) in [first, second].iter().enumerate() {
        if piece.is_empty() {
            continue;
        }
        if must_neutral().contains(piece.as_str()) || must_neutral().contains(tail2(piece).as_str()) {
            let at = if index == 0 { first_len } else { n } - 1;
            if at < finals.len() {
                finals[at] = with_tone(&finals[at], '5');
            }
        }
    }
}

fn bu_sandhi(word: &str, finals: &mut [String]) {
    let chars: Vec<char> = word.chars().collect();
    if chars.len() == 3 && chars[1] == BU {
        finals[1] = with_tone(&finals[1], '5');
    } else {
        for (i, c) in chars.iter().enumerate() {
            if *c == BU && i + 1 < chars.len() && tone_of(&finals[i + 1]) == '4' {
                finals[i] = with_tone(&finals[i], '2');
            }
        }
    }
}

fn yi_sandhi(word: &str, finals: &mut [String]) {
    let chars: Vec<char> = word.chars().collect();
    if word.contains(YI) && chars.iter().filter(|c| **c != YI).all(|c| c.is_numeric()) {
        return;
    }
    if chars.len() == 3 && chars[1] == YI && chars[0] == chars[2] {
        finals[1] = with_tone(&finals[1], '5');
    } else if word.starts_with("第一") {
        finals[1] = with_tone(&finals[1], '1');
    } else {
        for (i, c) in chars.iter().enumerate() {
            if *c == YI && i + 1 < chars.len() {
                if matches!(tone_of(&finals[i + 1]), '4' | '5') {
                    finals[i] = with_tone(&finals[i], '2');
                } else if !PUNC.contains(chars[i + 1]) {
                    finals[i] = with_tone(&finals[i], '4');
                }
            }
        }
    }
}

fn three_sandhi(word: &str, finals: &mut Vec<String>) {
    let n = word.chars().count();
    if n == 2 && all_tone_three(finals) {
        finals[0] = with_tone(&finals[0], '2');
    } else if n == 3 {
        let (first, _) = split_word(word);
        let first_len = first.chars().count();
        if all_tone_three(finals) {
            if first_len == 2 {
                finals[0] = with_tone(&finals[0], '2');
                finals[1] = with_tone(&finals[1], '2');
            } else if first_len == 1 {
                finals[1] = with_tone(&finals[1], '2');
            }
        } else {
            let mut parts = [finals[..first_len.min(n)].to_vec(), finals[first_len.min(n)..].to_vec()];
            for i in 0..2 {
                if parts[i].len() == 2 && all_tone_three(&parts[i]) {
                    parts[i][0] = with_tone(&parts[i][0], '2');
                } else if i == 1
                    && !all_tone_three(&parts[i])
                    && parts[i].first().map(|f| tone_of(f)) == Some('3')
                    && parts[0].last().map(|f| tone_of(f)) == Some('3')
                {
                    let at = parts[0].len() - 1;
                    parts[0][at] = with_tone(&parts[0][at], '2');
                }
            }
            *finals = parts.concat();
        }
    } else if n == 4 {
        let mut head = finals[..2].to_vec();
        let mut tail = finals[2..].to_vec();
        for sub in [&mut head, &mut tail] {
            if all_tone_three(sub) {
                sub[0] = with_tone(&sub[0], '2');
            }
        }
        head.extend(tail);
        *finals = head;
    }
}

/// Apply every sandhi rule to one word's finals, in misaki's order.
pub fn modified_tone(word: &str, pos: &str, finals: &mut Vec<String>) {
    if finals.len() != word.chars().count() || finals.is_empty() {
        return;
    }
    bu_sandhi(word, finals);
    yi_sandhi(word, finals);
    neutral_sandhi(word, pos, finals);
    three_sandhi(word, finals);
}

type Seg = Vec<(String, String)>;

fn tones(word: &str) -> Vec<String> {
    pinyin::syllables(word)
        .into_iter()
        .map(|s| s.and_then(|s| pinyin::split(s)).map(|p| format!("{}{}", p.final_, p.tone)).unwrap_or_else(|| "0".into()))
        .collect()
}

fn merge_bu(seg: Seg) -> Seg {
    let mut out = Seg::new();
    for (i, (word, pos)) in seg.iter().enumerate() {
        let mut word = word.clone();
        if !is_x_eng(pos) && i > 0 && seg[i - 1].0 == BU.to_string() {
            word = format!("{}{}", seg[i - 1].0, word);
        }
        let next_pos = seg.get(i + 1).map(|s| s.1.as_str());
        if word != BU.to_string() || next_pos.is_none() || next_pos.is_some_and(is_x_eng) {
            out.push((word, pos.clone()));
        }
    }
    out
}

fn merge_yi(seg: Seg) -> Seg {
    let mut out = Seg::new();
    let mut skip_next = false;
    for (i, (word, pos)) in seg.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if i >= 1
            && *word == YI.to_string()
            && i + 1 < seg.len()
            && seg[i - 1].0 == seg[i + 1].0
            && seg[i - 1].1 == "v"
            && !is_x_eng(&seg[i + 1].1)
        {
            let last = out.last_mut().unwrap();
            last.0 = format!("{}{}{}", last.0, YI, seg[i + 1].0);
            skip_next = true;
        } else {
            out.push((word.clone(), pos.clone()));
        }
    }
    let seg = out;
    let mut out = Seg::new();
    for (word, pos) in seg {
        if out.last().is_some_and(|l| l.0 == YI.to_string()) && !is_x_eng(&pos) {
            let last = out.last_mut().unwrap();
            last.0 = format!("{}{}", last.0, word);
        } else {
            out.push((word, pos));
        }
    }
    out
}

fn is_reduplication(word: &str) -> bool {
    let c: Vec<char> = word.chars().collect();
    c.len() == 2 && c[0] == c[1]
}

fn merge_three_tones(seg: Seg, whole_word: bool) -> Seg {
    let finals: Vec<Vec<String>> = seg.iter().map(|(w, p)| if is_x_eng(p) { vec!["0".into()] } else { tones(w) }).collect();
    let mut out = Seg::new();
    let mut merge_last = vec![false; seg.len()];
    for (i, (word, pos)) in seg.iter().enumerate() {
        let joins = !is_x_eng(pos)
            && i >= 1
            && !merge_last[i - 1]
            && if whole_word {
                all_tone_three(&finals[i - 1]) && all_tone_three(&finals[i])
            } else {
                finals[i - 1].last().map(|f| tone_of(f)) == Some('3') && finals[i].first().map(|f| tone_of(f)) == Some('3')
            };
        if joins && !is_reduplication(&seg[i - 1].0) && seg[i - 1].0.chars().count() + word.chars().count() <= 3 {
            out.last_mut().unwrap().0.push_str(word);
            merge_last[i] = true;
        } else {
            out.push((word.clone(), pos.clone()));
        }
    }
    out
}

fn merge_er(seg: Seg) -> Seg {
    let mut out = Seg::new();
    for (i, (word, pos)) in seg.iter().enumerate() {
        if i >= 1 && word == "儿" && out.last().is_some_and(|l| !is_x_eng(&l.1)) {
            out.last_mut().unwrap().0.push_str(word);
        } else {
            out.push((word.clone(), pos.clone()));
        }
    }
    out
}

fn merge_reduplication(seg: Seg) -> Seg {
    let mut out = Seg::new();
    for (word, pos) in seg {
        if out.last().is_some_and(|l| l.0 == word) && !is_x_eng(&pos) {
            out.last_mut().unwrap().0.push_str(&word);
        } else {
            out.push((word, pos));
        }
    }
    out
}

/// Merge neighbouring words so sandhi sees the units it needs.
pub fn pre_merge(seg: Seg) -> Seg {
    let seg = merge_bu(seg);
    let seg = merge_yi(seg);
    let seg = merge_reduplication(seg);
    let seg = merge_three_tones(seg, true);
    let seg = merge_three_tones(seg, false);
    merge_er(seg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(word: &str, pos: &str, finals: &[&str]) -> Vec<String> {
        let mut f: Vec<String> = finals.iter().map(|s| s.to_string()).collect();
        modified_tone(word, pos, &mut f);
        f
    }

    #[test]
    fn two_third_tones_become_second_then_third() {
        assert_eq!(run("你好", "l", &["i3", "ao3"]), ["i2", "ao3"]);
    }

    #[test]
    fn bu_and_yi() {
        assert_eq!(run("不怕", "v", &["u4", "a4"]), ["u2", "a4"]);
        assert_eq!(run("看不懂", "v", &["an4", "u4", "ong3"]), ["an4", "u5", "ong3"]);
        assert_eq!(run("一段", "m", &["i1", "uan4"]), ["i2", "uan4"]);
        assert_eq!(run("一天", "m", &["i1", "ian1"]), ["i4", "ian1"]);
    }

    #[test]
    fn neutral_tones() {
        assert_eq!(run("朋友", "n", &["eng2", "iou3"]), ["eng2", "iou5"]);
        assert_eq!(run("桌子", "n", &["uo1", "ii3"]), ["uo1", "ii5"]);
    }

    #[test]
    fn merges_prepare_words() {
        let seg = pre_merge(vec![("不".into(), "d".into()), ("好".into(), "a".into())]);
        assert_eq!(seg, vec![("不好".to_string(), "a".to_string())]);
        let seg = pre_merge(vec![("听".into(), "v".into()), ("一".into(), "m".into()), ("听".into(), "v".into())]);
        assert_eq!(seg, vec![("听一听".to_string(), "v".to_string())]);
    }
}
