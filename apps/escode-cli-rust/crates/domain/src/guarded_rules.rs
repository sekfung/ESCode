//! Guarded 规则：git / find / CMD 内建删除 / robocopy（TS guarded/git.ts、find.ts、windows-delete.ts）。

use crate::guarded::{
    COMMAND_SCAN, CommandWord, Dialect, OptionTable, Rule, WRAPPER_SCAN, enabled, parse_options,
    settle,
};

const HELP: &str = "-h --help";
const FORCE: [&str; 2] = ["-f", "--force"];
const TREE: [&str; 3] = [".", "./", ":/"];

fn common() -> String {
    format!("{HELP} -q --quiet -v --verbose")
}

fn has_help(flags: &[String]) -> bool {
    flags.iter().any(|flag| flag == "--help" || flag == "-h")
}

/// TS `matchGit`：Miss=已支持且未命中；Unknown=超出当前命令/选项文法。
pub(crate) fn match_git(words: &[CommandWord]) -> Rule {
    // 前置参数走 abort 策略，未知即整体 unsupported；-c 只按取值 arity 跳过，不解释配置内容。
    let front_table = OptionTable::new(
        "--no-pager -P --paginate -p --literal-pathspecs --glob-pathspecs --noglob-pathspecs --icase-pathspecs --bare --no-replace-objects --no-optional-locks",
        "-C --git-dir --work-tree --namespace -c --config-env --super-prefix",
        "--exec-path",
    );
    let front = parse_options(words, &front_table, WRAPPER_SCAN);
    if front.unsupported || front.operands.first().is_some_and(|word| word.dynamic) {
        return Rule::Unknown;
    }
    let Some((head, args)) = front.operands.split_first() else {
        return Rule::Miss;
    };
    let common = common();
    let table = match head.value.as_str() {
        "reset" => OptionTable::new(
            &format!(
                "{common} --hard --soft --mixed --merge --keep -p --patch -N --intent-to-add --no-refresh --refresh --recurse-submodules --no-recurse-submodules"
            ),
            "",
            "",
        ),
        "clean" => OptionTable::new(
            &format!("{common} -f --force -d -x -X -n --dry-run --no-dry-run -i --interactive"),
            "-e --exclude",
            "",
        ),
        "push" => OptionTable::new(
            &format!(
                "{common} -f --force --no-force --mirror --no-mirror -n --dry-run --no-dry-run --no-force-with-lease --force-if-includes --no-force-if-includes --all --branches --tags --delete -d --prune --atomic --no-atomic --porcelain --follow-tags --no-follow-tags --set-upstream -u --thin --no-thin --progress --no-progress --ipv4 -4 --ipv6 -6"
            ),
            "--repo --receive-pack --exec -o --push-option",
            "--force-with-lease --signed --recurse-submodules",
        ),
        "checkout" => OptionTable::new(
            &format!(
                "{common} -f --force --no-force --detach --ours --theirs -m --merge --conflict -p --patch --ignore-other-worktrees --recurse-submodules --no-recurse-submodules --no-overlay --overlay"
            ),
            "-b -B --orphan --conflict",
            "",
        ),
        "restore" => OptionTable::new(
            &format!(
                "{common} -W --worktree --no-worktree -S --staged --no-staged --ours --theirs -m --merge -p --patch --ignore-unmerged --overlay --no-overlay --recurse-submodules --no-recurse-submodules"
            ),
            "-s --source --conflict",
            "",
        ),
        "stash" => {
            if args.first().is_some_and(|word| word.dynamic) {
                return Rule::Unknown;
            }
            if args.first().map(|word| word.value.as_str()) != Some("clear") {
                return Rule::Miss;
            }
            let parsed = parse_options(&args[1..], &OptionTable::new(HELP, "", ""), COMMAND_SCAN);
            return settle(
                (!has_help(&parsed.flags)).then_some("git-stash-clear"),
                &parsed,
            );
        }
        "worktree" => {
            if args.first().is_some_and(|word| word.dynamic) {
                return Rule::Unknown;
            }
            if args.first().map(|word| word.value.as_str()) != Some("remove") {
                return Rule::Miss;
            }
            let parsed = parse_options(
                &args[1..],
                &OptionTable::new(&format!("{HELP} -f --force --no-force"), "", ""),
                COMMAND_SCAN,
            );
            let hit = !has_help(&parsed.flags) && enabled(&parsed.flags, &FORCE, &["--no-force"]);
            return settle(hit.then_some("git-worktree-force-remove"), &parsed);
        }
        _ => return Rule::Unknown,
    };
    let parsed = parse_options(args, &table, COMMAND_SCAN);
    let (flags, operands) = (&parsed.flags, &parsed.operands);
    // Git 的 -h/--help 是可靠的查询退出；未知选项不改变这一点，也不抹去已识别的 force/hard。
    if has_help(flags) {
        return Rule::Miss;
    }
    let dry_run = enabled(flags, &["-n", "--dry-run"], &["--no-dry-run"]);
    let force = enabled(flags, &FORCE, &["--no-force"]);
    let discards_tree = || {
        operands
            .iter()
            .any(|word| !word.dynamic && TREE.contains(&word.value.as_str()))
    };
    let hit = match head.value.as_str() {
        "reset" => enabled(
            flags,
            &["--hard"],
            &["--soft", "--mixed", "--merge", "--keep", "-p", "--patch"],
        )
        .then_some("git-reset-hard"),
        "clean" => (!dry_run && force).then_some("git-clean-force"),
        "push" => {
            let refspecs = if flags.iter().any(|flag| flag == "--repo") {
                &operands[..]
            } else {
                operands.get(1..).unwrap_or_default()
            };
            // glob 会让整词标为 dynamic，但已知的 + 前缀仍是强推证据。
            (!dry_run
                && (force
                    || enabled(flags, &["--force-with-lease"], &["--no-force-with-lease"])
                    || enabled(flags, &["--mirror"], &["--no-mirror"])
                    || refspecs.iter().any(|word| word.value.starts_with('+'))))
            .then_some("git-push-force")
        }
        "checkout" => (force || discards_tree()).then_some("git-discard-tree"),
        "restore" => {
            let staged_only = enabled(flags, &["-S", "--staged"], &["--no-staged"])
                && !enabled(flags, &["-W", "--worktree"], &["--no-worktree"]);
            (!staged_only && discards_tree()).then_some("git-discard-tree")
        }
        _ => None,
    };
    settle(hit, &parsed)
}

const FIND_FLAGS: &str = "-H -L -P -O0 -O1 -O2 -O3 -depth -d -x -xdev -mount -ignore_readdir_race -noignore_readdir_race -daystart -follow -noleaf -true -false -empty -readable -writable -executable -print -print0 -ls -prune -quit -a -and -o -or -not ! ( ) ,";
const FIND_VALUES: &str = "-D -f -name -iname -path -ipath -wholename -iwholename -regex -iregex -regextype -type -xtype -size -user -group -uid -gid -perm -links -inum -samefile -newer -anewer -cnewer -atime -ctime -mtime -amin -cmin -mmin -used -maxdepth -mindepth -printf -fprint -fprint0 -fls -files0-from";
const FIND_EXEC: [&str; 4] = ["-exec", "-execdir", "-ok", "-okdir"];

fn is_newer_xy(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.len() == 8
        && value.starts_with("-newer")
        && b"acmBt".contains(&bytes[6])
        && b"acmBt".contains(&bytes[7])
}

/// TS `matchFindDelete`：有限 find 表达式；未知 arity 后不把参数值猜成动作，不递归分析 exec 载荷。
pub(crate) fn match_find_delete(args: &[CommandWord]) -> Rule {
    let mut expression = false;
    let mut i = 0;
    while i < args.len() {
        let word = &args[i];
        let value = word.value.as_str();
        if !expression && !value.starts_with('-') && !["!", "("].contains(&value) {
            i += 1;
            continue;
        }
        if word.dynamic {
            return Rule::Unknown;
        }
        // BSD 的 -d/-x 可位于起始路径前，不能提前进入表达式阶段而把后续路径当未知谓词。
        if !expression
            && ["-H", "-L", "-P", "-O0", "-O1", "-O2", "-O3", "-d", "-x"].contains(&value)
        {
            i += 1;
            continue;
        }
        if !expression && ["-D", "-f"].contains(&value) {
            i += 1;
            if i >= args.len() {
                return Rule::Unknown;
            }
            i += 1;
            continue;
        }
        expression = true;
        if value == "-delete" {
            return Rule::Hit("find-delete");
        }
        if FIND_EXEC.contains(&value) {
            let mut closed = false;
            i += 1;
            while i < args.len() {
                let token = &args[i];
                if !token.dynamic
                    && (token.value == ";"
                        || (token.value == "+"
                            && args[i - 1].value == "{}"
                            && ["-exec", "-execdir"].contains(&value)))
                {
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                return Rule::Unknown;
            }
        } else if value == "-fprintf"
            || FIND_VALUES.split(' ').any(|known| known == value)
            || is_newer_xy(value)
        {
            i += if value == "-fprintf" { 2 } else { 1 };
            if i >= args.len() {
                return Rule::Unknown;
            }
        } else if !FIND_FLAGS.split(' ').any(|known| known == value) {
            return Rule::Unknown;
        }
        i += 1;
    }
    Rule::Miss
}

fn is_attribute_switch(option: &str) -> bool {
    // TS `/^A(?::?[-RASHIO]*)$/`
    let Some(rest) = option.strip_prefix('A') else {
        return false;
    };
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    rest.chars().all(|c| "-RASHIO".contains(c))
}

/// TS `matchCmdDelete`：斜杠开关只在 CMD 分支解释；独立帮助不是删除调用。
pub(crate) fn match_cmd_delete(name: &str, args: &[CommandWord]) -> Rule {
    if args.len() == 1 && args[0].value == "/?" && !args[0].dynamic {
        return Rule::Miss;
    }
    let tree = name == "rd" || name == "rmdir";
    let (mut recursive, mut target, mut unsupported) = (false, false, false);
    for word in args {
        let value = word.value.to_uppercase();
        if !value.starts_with('/') || word.dynamic {
            target = true;
            continue;
        }
        for option in value[1..].split('/') {
            if option == "S" {
                recursive = true;
            } else if !(["Q", "?"].contains(&option)
                || (!tree && ["P", "F"].contains(&option))
                || (!tree && is_attribute_switch(option)))
            {
                unsupported = true;
            }
        }
    }
    if target && (!tree || recursive) {
        return Rule::Hit(if tree {
            "cmd-remove-tree"
        } else {
            "cmd-delete-files"
        });
    }
    if unsupported {
        Rule::Unknown
    } else {
        Rule::Miss
    }
}

const ROBOCOPY_FLAGS: &str = "/MIR /PURGE /L /S /E /Z /B /ZB /J /EFSRAW /SEC /COPYALL /NOCOPY /SECFIX /TIMFIX /CREATE /FAT /256 /PF /SJ /SL /NODCOPY /NOOFFLOAD /COMPRESS /A /M /XC /XN /XO /XX /XL /IM /IS /IT /XJ /FFT /DST /XJD /XJF /TBD /LFSM /X /V /TS /FP /BYTES /NS /NC /NFL /NDL /NP /ETA /TEE /NJH /NJS /UNICODE /NP /?";
const ROBOCOPY_VALUES: &str = "/LEV /COPY /DCOPY /A+ /A- /MON /MOT /RH /IPG /MT /IOMAXSIZE /IORATE /THRESHOLD /IA /XA /MAX /MIN /MAXAGE /MINAGE /MAXLAD /MINLAD /R /W /LFSM /LOG /LOG+ /UNILOG /UNILOG+";

/// TS `matchRobocopyDelete`：值选项用冒号绑定；XF/XD 列表遇到下一个斜杠开关结束。
pub(crate) fn match_robocopy_delete(args: &[CommandWord], dialect: Dialect) -> Rule {
    if args.len() < 2 {
        return Rule::Miss;
    }
    let (mut deletion, mut list_only, mut paths, mut exclusions) = (false, false, 0, false);
    let delete_or_unknown = |deletion: bool| {
        if deletion {
            Rule::Hit("robocopy-delete")
        } else {
            Rule::Unknown
        }
    };
    for word in args {
        let value = word.value.to_uppercase();
        if word.dynamic {
            return delete_or_unknown(deletion);
        }
        // Git Bash 会把 /c/... 转为 Windows 盘符路径；这是 argv 语法，不是未知斜杠开关。
        let bytes = word.value.as_bytes();
        let drive_path = dialect == Dialect::GitBash
            && bytes.len() >= 3
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && bytes[2] == b'/';
        if !value.starts_with('/') || drive_path {
            if !exclusions {
                paths += 1;
            }
            continue;
        }
        exclusions = value == "/XF" || value == "/XD";
        if value == "/MIR" || value == "/PURGE" {
            deletion = true;
        } else if value == "/L" {
            list_only = true;
        } else if value == "/XF"
            || value == "/XD"
            || ROBOCOPY_FLAGS.split(' ').any(|known| known == value)
            || (value.contains(':')
                && ROBOCOPY_VALUES
                    .split(' ')
                    .any(|known| Some(known) == value.split(':').next()))
        {
            continue;
        } else {
            // JOB 与未知 arity 后不能生成豁免，也不能撤销之前明确定位的删除。
            return delete_or_unknown(deletion);
        }
    }
    if paths >= 2 && deletion && !list_only {
        Rule::Hit("robocopy-delete")
    } else {
        Rule::Miss
    }
}
