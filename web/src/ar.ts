// The Arabic page's texts, keyed by the English text they translate
// (i18n.tsx). A change to an English text in content.ts, App.tsx,
// Diagrams.tsx or head.ts needs its entry here changed with it: the
// build fails naming each English text that has no Arabic. Names of
// programs, APIs and code stay in Latin script, inside the Arabic.
export const ar: Record<string, string> = {
  "gala — a Rust kernel that runs Windows programs":
    "gala — نواة بلغة Rust تشغّل برامج Windows",
  "gala is a from-scratch x86_64 operating system kernel in Rust that runs unmodified Windows programs on its own clean-room system DLLs, with a Linux personality on the same core.":
    "gala نواة نظام تشغيل x86_64 مكتوبة من الصفر بلغة Rust، تشغّل برامج Windows دون تعديل على مكتبات DLL للنظام مكتوبة من الصفر بأسلوب الغرفة النظيفة، مع شخصية Linux على النواة نفسها.",
  "A from-scratch x86_64 kernel in Rust. Unmodified Windows programs, clean-room system DLLs, a Linux personality on the same core.":
    "نواة x86_64 مكتوبة من الصفر بلغة Rust. برامج Windows دون تعديل، ومكتبات DLL للنظام بأسلوب الغرفة النظيفة، وشخصية Linux على النواة نفسها.",
  "gala's desktop with the start menu open: pinned Windows apps, and Ubuntu under Linux distributions.":
    "سطح مكتب gala وقائمة ابدأ مفتوحة: تطبيقات Windows مثبّتة، وUbuntu ضمن توزيعات Linux.",
  "Skip to content":
    "انتقل إلى المحتوى",
  "Sections":
    "الأقسام",
  "How it works":
    "كيف يعمل",
  "Beyond a reboot":
    "ما بعد إعادة التشغيل",
  "Components":
    "المكوّنات",
  "In progress":
    "قيد العمل",
  "What runs":
    "ما الذي يعمل",
  "Tooling":
    "الأدوات",
  "Try it":
    "جرّبه",
  "From a USB stick":
    "من ذاكرة USB",
  "A from-scratch operating system in Rust":
    "نظام تشغيل مكتوب من الصفر بلغة Rust",
  "Runs Windows programs. Owes Windows nothing underneath.":
    "يشغّل برامج Windows، ولا يدين لـ Windows بشيء في أعماقه.",
  "What runs on it":
    "ما الذي يعمل عليه",
  "Download":
    "التنزيل",
  "Boot it from USB":
    "أقلِعه من ذاكرة USB",
  "gala's desktop with the start menu open: pinned apps, and Ubuntu under Linux distributions.":
    "سطح مكتب gala وقائمة ابدأ مفتوحة: تطبيقات مثبّتة، وUbuntu ضمن توزيعات Linux.",
  "An operating system is built in layers. A program asks the layer below for what it needs, and the request travels down until something can do the work. gala's trick is at one particular layer: the system call boundary.":
    "يُبنى نظام التشغيل على طبقات. يطلب البرنامج ما يحتاجه من الطبقة التي تحته، وينتقل الطلب نزولًا حتى يصل إلى ما يستطيع تنفيذه. وسرّ gala يكمن في طبقة بعينها: حدّ استدعاءات النظام.",
  "Why the system call boundary?":
    "لماذا حدّ استدعاءات النظام؟",
  "Why not copy Windows' internals?":
    "لماذا لا تُنسخ البنية الداخلية لـ Windows؟",
  "Because they aren't needed and aren't ours to copy. Below the boundary, gala's memory manager, scheduler, file system and drivers are its own design. That leaves them free to change, for example to be replaced while programs are still running.":
    "لأنها غير ضرورية، وليس لنا أن ننسخها. تحت هذا الحدّ، مدير الذاكرة والمُجدول ونظام الملفات والمُشغّلات في gala كلها من تصميمها الخاص، فتبقى حرّة في التغيّر، كأن تُستبدل مثلًا بينما البرامج لا تزال تعمل.",
  "Where does Linux fit?":
    "أين يقع Linux من ذلك؟",
  "A personality is a translation layer from one system's calls to the core. gala has two. Linux programs bring their own libraries (glibc, GTK) and make Linux system calls, and the Linux layer answers them with the same core the Windows layer uses.":
    "الشخصية طبقة ترجمة من استدعاءات نظام ما إلى النواة. لدى gala شخصيتان. تجلب برامج Linux مكتباتها الخاصة (glibc وGTK) وتُجري استدعاءات نظام Linux، فتجيب عنها طبقة Linux بالنواة نفسها التي تستخدمها طبقة Windows.",
  "One system call, step by step":
    "استدعاء نظام واحد، خطوة بخطوة",
  "What happens when a program reads a file:":
    "ما يحدث حين يقرأ برنامج ملفًا:",
  "The path of a ReadFile call through gala":
    "مسار استدعاء ReadFile عبر gala",
  "ReadFile":
    "ReadFile",
  "the program calls kernel32.dll":
    "يستدعي البرنامج kernel32.dll",
  "NtReadFile":
    "NtReadFile",
  "kernel32 calls into ntdll.dll":
    "تستدعي kernel32 مكتبة ntdll.dll",
  "syscall":
    "syscall",
  "ntdll puts the call's number in a register and enters the kernel":
    "تضع ntdll رقم الاستدعاء في سجلّ المعالج وتدخل النواة",
  "Entry and dispatch":
    "الدخول والتوزيع",
  "registers saved; the number picks the service":
    "تُحفظ سجلّات المعالج، ويحدّد الرقم الخدمة المطلوبة",
  "Handle and access":
    "المقبض والصلاحيات",
  "the handle names a file object; the caller's token is checked":
    "يشير المقبض إلى كائن ملف، ويُتحقّق من رمز الوصول الخاص بالمستدعي",
  "File system":
    "نظام الملفات",
  "the data comes from C:, through the disk driver if needed":
    "تأتي البيانات من C:، عبر مُشغّل القرص عند الحاجة",
  "Back":
    "العودة",
  "a status code returns and the program carries on":
    "يُعاد رمز الحالة ويتابع البرنامج عمله",
  "Every system call also feeds gala's own diagnostics. A function that gala lacks is listed in a ledger at the end of the run, so no gap stays hidden.":
    "كل استدعاء نظام يغذّي أيضًا أدوات التشخيص في gala. وأي دالة تفتقدها gala تُدرج في سجلّ حصر عند نهاية التشغيل، فلا تبقى أي ثغرة خفية.",
  "From power-on to the first program":
    "من التشغيل إلى أول برنامج",
  "The boot sequence":
    "تسلسل الإقلاع",
  "Firmware":
    "البرنامج الثابت",
  "UEFI or BIOS":
    "UEFI أو BIOS",
  "Limine":
    "Limine",
  "loads the kernel and its files":
    "يحمّل النواة وملفاتها",
  "Kernel start":
    "بدء النواة",
  "memory, clock, every CPU":
    "الذاكرة، والساعة، وكل المعالجات",
  "Drivers":
    "المُشغّلات",
  "disk, network, screen":
    "القرص، والشبكة، والشاشة",
  "C: assembled":
    "تجميع C:",
  "DLLs, registry, user profiles":
    "مكتبات DLL، والسجل، وملفات تعريف المستخدمين",
  "Programs":
    "البرامج",
  "services, then the desktop":
    "الخدمات، ثم سطح المكتب",
  "Two systems, one desktop":
    "نظامان، وسطح مكتب واحد",
  "Most systems lose everything that is running when the kernel changes or the machine stops. gala is built so that running state can outlive both: written down, carried over and started again.":
    "تفقد معظم الأنظمة كل ما يعمل حين تتغيّر النواة أو يتوقف الجهاز. أما gala فمبنيّة بحيث تتجاوز الحالة الجارية كليهما: تُدوَّن، وتُنقل، ثم تُستأنف.",
  "The running kernel hands the machine to a new kernel image, without the firmware or the bootloader, and the programs carry on: their memory stays where it is, threads blocked in waits and I/O are reissued, and open TCP and UDP connections survive.":
    "تسلّم النواة الجارية الجهاز إلى صورة نواة جديدة دون المرور بالبرنامج الثابت أو مُحمّل الإقلاع، وتتابع البرامج عملها: تبقى ذاكرتها في مكانها، وتُعاد إصدار عمليات الانتظار والإدخال/الإخراج للخيوط المحجوبة، وتبقى اتصالات TCP وUDP المفتوحة قائمة.",
  "Every thread is stopped at a quiescent point and the kernel's state is written as a versioned record. The new kernel can be a newer build with a different state format; what it cannot carry yet makes the handover refuse with the reason, never half-happen.":
    "يُوقَف كل خيط عند نقطة سكون، وتُكتب حالة النواة سجلًّا ذا إصدار. يمكن أن تكون النواة الجديدة بناءً أحدث بصيغة حالة مختلفة؛ وما لا تستطيع نقله بعد يجعل التسليم يُرفض مع ذكر السبب، ولا يحدث نصفه أبدًا.",
  "A running machine is saved when a chosen line appears in its log and resumed from there, as often as wanted. One snapshot can be resumed twice to try two fixes from the same state.":
    "يُحفظ الجهاز الجاري حين يظهر سطر مختار في سجل الأحداث، ويُستأنف من هناك كلما شئت. ويمكن استئناف اللقطة الواحدة مرتين لتجربة إصلاحين من الحالة نفسها.",
  "The Office install takes 28 minutes to reach the step that fails. Saved there, it resumes in 3 seconds and reaches the end 21 seconds later. A snapshot only resumes on the build it was taken with, and every difference is named.":
    "يستغرق تثبيت Office ‏28 دقيقة ليبلغ الخطوة التي يفشل فيها. وإذا حُفظ هناك، يُستأنف في 3 ثوانٍ ويبلغ النهاية بعد 21 ثانية. لا تُستأنف اللقطة إلا على البناء الذي أُخذت به، ويُسمّى كل اختلاف.",
  "A fix to a DLL or to the kernel is patched into a resumed snapshot, into every process that has it loaded, without starting over.":
    "يُرقَّع إصلاحُ مكتبة DLL أو النواة داخل لقطة مُستأنفة، في كل عملية حمّلتها، دون البدء من جديد.",
  "The old and new builds are compared function by function. Every program thread is stopped, no changed function may be on any thread's stack, the new code is mapped beside the old and each old entry jumps to its new version. Anything unsafe is refused with the reason. It is a development tool: a hot-swapped run is never counted as a pass.":
    "يُقارَن البناءان القديم والجديد دالةً دالة. يُوقَف كل خيط في البرامج، ولا يجوز أن تكون أي دالة متغيّرة على مكدّس أي خيط، وتُربط الشيفرة الجديدة بجوار القديمة ويقفز كل مدخل قديم إلى نسخته الجديدة. وكل ما ليس آمنًا يُرفض مع ذكر السبب. إنها أداة تطوير: التشغيل المبدَّل حيًّا لا يُحتسب نجاحًا أبدًا.",
  "A running gala serving SSH and Nginx updates its system image as well as its kernel, and every connection and session goes on.":
    "نظام gala جارٍ يخدم SSH وNginx يحدّث صورة نظامه ونواته معًا، وتستمر كل الاتصالات والجلسات.",
  "The kernel half is the handover above. What it adds: carrying consoles, jobs, pipes in mid-request and the desktop across a handover, and restarting user-mode processes one at a time onto a new system image.":
    "نصف النواة هو التسليم الموصوف أعلاه. وما يضيفه هذا: نقل وحدات التحكم والمهام والأنابيب في منتصف الطلب وسطح المكتب عبر التسليم، وإعادة تشغيل عمليات وضع المستخدم واحدة تلو الأخرى على صورة نظام جديدة.",
  "gala snapshot freezes one program and its children; gala restore starts it again, here or, exported as one file, on another machine, where its windows open on the new desktop.":
    "يُجمّد gala snapshot برنامجًا واحدًا وأبناءه؛ ويعيد gala restore تشغيله، هنا أو على جهاز آخر بعد تصديره ملفًّا واحدًا، فتُفتح نوافذه على سطح المكتب الجديد.",
  "A program snapshot is the handover's record cut down to one process tree: its memory, threads, handles, windows and sockets. Exports are signed and encrypted to their target, since a program's memory holds its secrets.":
    "لقطة البرنامج هي سجلّ التسليم مقتصرًا على شجرة عمليات واحدة: ذاكرتها وخيوطها ومقابضها ونوافذها ومقابسها. وتُوقَّع الصادرات وتُشفَّر لوجهتها، لأن ذاكرة البرنامج تحوي أسراره.",
  "A kernel handover, step by step":
    "تسليم النواة، خطوة بخطوة",
  "How a new kernel takes over a running machine. The programs see a pause, not a restart.":
    "كيف تتسلّم نواة جديدة جهازًا جاريًا. ترى البرامج توقفًا مؤقتًا، لا إعادة تشغيل.",
  "The steps of a kernel handover":
    "خطوات تسليم النواة",
  "Load the new kernel":
    "تحميل النواة الجديدة",
  "its image is checked against its signature first":
    "يُتحقّق من صورتها مقابل توقيعها أولًا",
  "Quiesce":
    "التسكين",
  "every thread stops where it can safely stop; a blocked call is noted for later":
    "يتوقف كل خيط حيث يمكنه التوقف بأمان؛ ويُدوَّن الاستدعاء المحجوب لوقت لاحق",
  "Write the record":
    "كتابة سجلّ الحالة",
  "processes, memory, handles, sockets, the clock: versioned data":
    "العمليات، والذاكرة، والمقابض، والمقابس، والساعة: بيانات ذات إصدار",
  "Jump":
    "القفز",
  "straight into the new kernel, no firmware or bootloader":
    "مباشرةً إلى النواة الجديدة، دون البرنامج الثابت أو مُحمّل الإقلاع",
  "Rebuild":
    "إعادة البناء",
  "the record is read, older formats too; memory stays in place":
    "يُقرأ سجلّ الحالة، بصيغه الأقدم أيضًا؛ وتبقى الذاكرة في مكانها",
  "Carry on":
    "المتابعة",
  "blocked calls are reissued and connections stay open":
    "يُعاد إصدار الاستدعاءات المحجوبة وتبقى الاتصالات مفتوحة",
  "Anything the record cannot carry yet makes the handover refuse, and the old kernel keeps running, with the reason in its log.":
    "كل ما لا يستطيع سجلّ الحالة نقله بعد يجعل التسليم يُرفض، وتواصل النواة القديمة عملها، مع ذكر السبب في سجل أحداثها.",
  "Next: a program that moves between machines":
    "التالي: برنامج ينتقل بين الأجهزة",
  "The same record, cut down to one program. Planned; the commands are those the roadmap names.":
    "سجلّ الحالة نفسه، مقتصرًا على برنامج واحد. مُخطّط؛ والأوامر هي التي تسمّيها خارطة الطريق.",
  "Freezing a program and starting it on another machine":
    "تجميد برنامج وتشغيله على جهاز آخر",
  "gala snapshot":
    "gala snapshot",
  "the program and its children freeze at a quiet point":
    "يتجمّد البرنامج وأبناؤه عند نقطة هادئة",
  "export":
    "export",
  "one file: memory, threads, handles, windows; signed and encrypted":
    "ملف واحد: الذاكرة، والخيوط، والمقابض، والنوافذ؛ موقّع ومشفّر",
  "import":
    "import",
  "version, digests and signature checked":
    "يُتحقّق من الإصدار والملخّصات والتوقيع",
  "gala restore":
    "gala restore",
  "it runs on, its windows on the new desktop":
    "يتابع عمله، ونوافذه على سطح المكتب الجديد",
  "Making rare bugs ordinary":
    "جعل الأخطاء النادرة عادية",
  "The same idea pointed at bugs: a failure is made to happen on demand, and when it happens, it explains itself.":
    "الفكرة نفسها موجّهة إلى الأخطاء: يُستحضر الفشل عند الطلب، وحين يحدث يشرح نفسه.",
  "One thread runs at a time, a seeded random choice picks which, and time is virtual. A race that shows up once repeats every time from its seed.":
    "يعمل خيط واحد في كل مرة، ويختار خيارٌ عشوائي ذو بذرة أيّها، والوقت افتراضي. وأي تسابق يظهر مرة واحدة يتكرر في كل مرة انطلاقًا من بذرته.",
  "The approach FoundationDB and TigerBeetle use, applied to Windows programs in gala-um.":
    "النهج الذي تتبعه FoundationDB وTigerBeetle، مطبّقًا على برامج Windows في gala-um.",
  "A run's boot time, clock readings and random numbers are recorded and played back. The clock can be set and moved while programs run, their timers following it, and the disk can be made slow on purpose.":
    "يُسجَّل وقت إقلاع التشغيل وقراءات الساعة والأرقام العشوائية ثم تُعاد. ويمكن ضبط الساعة وتحريكها أثناء عمل البرامج فتتبعها مؤقّتاتها، ويمكن إبطاء القرص عمدًا.",
  "Wrappers at the device and service boundaries (the clock, the random source, the disk), so a test can put the system in a state that is hard to reach for real.":
    "أغلفة عند حدود الأجهزة والخدمات (الساعة، ومصدر العشوائية، والقرص)، كي يضع الاختبار النظام في حالة يصعب بلوغها فعليًا.",
  "Memory allocations, physical pages, disk requests, network sends and page-file writes fail on purpose, every nth time or at random from a seed.":
    "تفشل عمليات تخصيص الذاكرة والصفحات الفعلية وطلبات القرص وعمليات الإرسال الشبكي والكتابة إلى ملف الصفحات عمدًا، كل n مرة أو عشوائيًا انطلاقًا من بذرة.",
  "Every allocation in the kernel can fail, so every failure path is exercised rather than trusted.":
    "كل عملية تخصيص في النواة قابلة للفشل، فيُختبر كل مسار فشل بدل الوثوق به.",
  "Every system call, information class, flag or DLL function a program reached that gala does not implement yet is counted and listed at the end of the run. No gap stays hidden behind a crash elsewhere.":
    "كل استدعاء نظام أو فئة معلومات أو راية أو دالة DLL يصل إليها برنامج ولم تُنفّذها gala بعد يُحصى ويُدرج في نهاية التشغيل. لا تبقى أي ثغرة مخفية خلف انهيار في موضع آخر.",
  "Ranked by how often each was hit, the list is the work queue for the next program.":
    "مرتّبةً حسب عدد مرات الوصول إلى كل منها، تصبح القائمة جدول العمل للبرنامج التالي.",
  "A stuck CPU is caught by a watchdog, and a crash writes its message, backtrace and last events to the disk, where the next boot finds and prints them.":
    "يرصد مؤقت مراقبة أي معالج عالق، ويكتب الانهيار رسالته وتتبّع المكدس وآخر الأحداث على القرص، حيث يجدها الإقلاع التالي ويطبعها.",
  "An always-on flight recorder keeps each CPU's last 256 system calls and exits, at the cost of a few stores each.":
    "يحتفظ مسجّل رحلة يعمل دائمًا بآخر 256 استدعاء نظام وخروجًا لكل معالج، بكلفة بضع عمليات كتابة لكل منها.",
  "Two locks taken in opposite orders on two paths are reported the first time the inversion is seen, before they ever deadlock.":
    "يُبلَّغ عن قفلين يُؤخذان بترتيبين متعاكسين في مسارين منذ أول مرة يُرصد فيها هذا الانعكاس، قبل أن يتسبّبا في أي جمود.",
  "On in debug kernels, host tests and gala-um, so CI catches an inversion even when the timing never lines up.":
    "مُفعّل في نوى التنقيح واختبارات المضيف وgala-um، فيكشف CI الانعكاس حتى حين لا يتوافق التوقيت أبدًا.",
  "The components":
    "المكوّنات",
  "The pieces gala is built from, layer by layer. Where a piece is partly done, the done part and the rest are listed apart.":
    "الأجزاء التي بُنيت منها gala، طبقةً طبقة. حين يكتمل جزء جزئيًا، يُدرج القسم المكتمل والباقي كلٌّ على حدة.",
  "The kernel core":
    "لبّ النواة",
  "gala's own design, owing nothing to Windows. Everything above it is built on these pieces, and they are free to change.":
    "تصميم gala الخاص، لا يدين لـ Windows بشيء. كل ما فوقه مبني على هذه الأجزاء، وهي حرّة في التغيّر.",
  "Limine loads the kernel from a hybrid image that starts on UEFI and BIOS machines, from a CD, a disk or a USB stick.":
    "يحمّل Limine النواة من صورة هجينة تُقلع على أجهزة UEFI وBIOS، من قرص مضغوط أو قرص صلب أو ذاكرة USB.",
  "Everything processor-specific sits behind one interface: x86_64 today, aarch64 being built behind the same seam.":
    "كل ما يخص المعالج يقبع خلف واجهة واحدة: x86_64 اليوم، وaarch64 قيد البناء خلف الفاصل نفسه.",
  "Physical frames, page tables, address spaces with demand-zero paging, guard pages and copy-on-write. Every allocation can fail without the kernel panicking.":
    "الإطارات الفيزيائية وجداول الصفحات وفضاءات العناوين مع ترحيل الصفحات الصفرية عند الطلب والصفحات الحارسة والنسخ عند الكتابة. يمكن لأي تخصيص أن يفشل دون أن تنهار النواة.",
  "Kernel stacks, context switches and the scheduler, on every CPU, with priorities and affinity as Windows programs set them.":
    "مكدسات النواة وتبديل السياق والمُجدول، على كل معالج، مع الأولويات والتقارب كما تضبطها برامج Windows.",
  "The CPU's counter is calibrated against a reference timer and checked against the real-time clock, so the whole system agrees on the time.":
    "يُعايَر عدّاد المعالج مقابل مؤقت مرجعي ويُتحقّق منه مقابل ساعة الوقت الحقيقي، فيتّفق النظام كله على الوقت.",
  "C: as a case-insensitive file system with Windows' share modes, kept on a virtio disk as a log that is replayed at the next boot, written back lazily.":
    "C: نظام ملفات غير حسّاس لحالة الأحرف مع أنماط المشاركة في Windows، محفوظ على قرص virtio كسجل يُعاد تنفيذه عند الإقلاع التالي، ويُكتب بشكل مؤجّل.",
  "A virtio network card and a TCP/IP stack (smoltcp), with loopback. TCP and UDP, IPv4 and the IPv6 loopback.":
    "بطاقة شبكة virtio ومكدس TCP/IP ‏(smoltcp)، مع الحلقة الراجعة. TCP وUDP، وIPv4 والحلقة الراجعة في IPv6.",
  "The bootloader's framebuffer or a virtio-gpu, a PS/2 keyboard and mouse.":
    "مخزن إطارات مُحمّل الإقلاع أو virtio-gpu، ولوحة مفاتيح وفأرة PS/2.",
  "The running kernel hands the machine to a new kernel image without a reboot, a newer build included, and the programs keep running: blocked threads are reissued and open TCP and UDP connections survive.":
    "تسلّم النواة العاملة الجهاز إلى صورة نواة جديدة دون إعادة تشغيل، حتى لو كانت إصدارًا أحدث، وتستمر البرامج في العمل: تُعاد الطلبات المعلّقة للخيوط المحجوبة وتبقى اتصالات TCP وUDP المفتوحة.",
  "Consoles, jobs, pipe and completion-port requests in flight, the desktop and its windows, IPv6 and raw sockets. While a program uses one of these, a handover refuses and says why.":
    "وحدات التحكم والمهام وطلبات الأنابيب ومنافذ الإكمال الجارية، وسطح المكتب ونوافذه، وIPv6 والمقابس الخام. ما دام برنامج يستخدم أحدها، يرفض التسليم ويذكر السبب.",
  "Every boot file is checked against its hash, an Ed25519-signed manifest is checked by the kernel and before every handover, and a rollback floor refuses older images.":
    "يُتحقّق من كل ملف إقلاع مقابل بصمته، وتتحقّق النواة من بيان موقّع بـ Ed25519 قبل كل تسليم أيضًا، ويرفض حدّ أدنى للتراجع الصور الأقدم.",
  "The chain of trust started from the firmware's own keys, and a release signing key.":
    "سلسلة الثقة بدءًا من مفاتيح البرنامج الثابت نفسه، ومفتاح لتوقيع الإصدارات.",
  "The Windows personality":
    "شخصية Windows",
  "This layer makes programs believe they are on Windows 11. It reproduces what programs can observe at the system call boundary, and nothing of Windows' internals.":
    "تجعل هذه الطبقة البرامج تعتقد أنها تعمل على Windows 11. فهي تعيد إنتاج ما تلاحظه البرامج عند حدّ استدعاءات النظام، ولا شيء من داخليات Windows.",
  "Windows' system calls on top of the core: objects and handles, processes, threads and jobs, virtual memory and sections, files and pipes, waits and timers, consoles.":
    "استدعاءات نظام Windows فوق اللبّ: الكائنات والمقابض، والعمليات والخيوط والمهام، والذاكرة الافتراضية والأقسام، والملفات والأنابيب، والانتظار والمؤقتات، ووحدات التحكم.",
  "ntdll, kernel32, user32, gdi32, ws2_32, ole32, shell32, crypt32 and more than eighty others, written from documentation. Programs load the libraries they expect.":
    "ntdll وkernel32 وuser32 وgdi32 وws2_32 وole32 وshell32 وcrypt32 وأكثر من ثمانين مكتبة أخرى، مكتوبة انطلاقًا من التوثيق. تحمّل البرامج المكتبات التي تتوقعها.",
  "Keys, values, notifications and access checks, seeded at boot and kept on disk.":
    "المفاتيح والقيم والإشعارات وفحوص الوصول، تُهيّأ عند الإقلاع وتُحفظ على القرص.",
  "Access tokens, security descriptors, access checks on every open, LSA logons, user profiles, sessions, integrity levels.":
    "رموز الوصول وواصفات الأمان وفحوص الوصول عند كل فتح، وتسجيلات دخول LSA، وملفات تعريف المستخدمين، والجلسات، ومستويات السلامة.",
  "A service control manager: services start at boot, are restarted when they die, and answer their installers' calls.":
    "مدير للتحكم في الخدمات: تبدأ الخدمات عند الإقلاع، ويُعاد تشغيلها حين تتوقف، وتستجيب لاستدعاءات مثبّتيها.",
  "Schannel with TLS 1.2 and 1.3, client and server, plus winhttp, wininet and the certificate chain engine.":
    "Schannel مع TLS 1.2 و1.3، للعميل والخادم، إضافةً إلى winhttp وwininet ومحرّك سلاسل الشهادات.",
  "x86 programs run as on 64-bit Windows, through a 32-bit ntdll, kernel32, C runtime, user32 and gdi32.":
    "تعمل برامج x86 كما على Windows بمعمارية 64 بت، عبر ntdll وkernel32 ومكتبة تشغيل C وuser32 وgdi32 بمعمارية 32 بت.",
  "GDI, Direct2D's shapes and WIC's images drawn in software, OpenGL and Vulkan on Mesa's llvmpipe and lavapipe, shown through a virtio-gpu.":
    "GDI وأشكال Direct2D وصور WIC مرسومة برمجيًا، وOpenGL وVulkan على llvmpipe وlavapipe من Mesa، معروضة عبر virtio-gpu.",
  "Direct3D over Vulkan through DXVK and vkd3d-proton, in software first. Direct3D 11 works through DXVK on lavapipe: a device and a swap chain clear, copy and present into a window on the desktop. Drawing with shaders comes next, then Direct3D 9, 10 and 12.":
    "Direct3D فوق Vulkan عبر DXVK وvkd3d-proton، برمجيًا أولًا. يعمل Direct3D 11 عبر DXVK على lavapipe: جهاز وسلسلة تبديل يمسحان وينسخان ويعرضان داخل نافذة على سطح المكتب. يليه الرسم بالمظلّلات، ثم Direct3D 9 و10 و12.",
  "Drawing and presenting on a real or virtual graphics card instead of the processor.":
    "الرسم والعرض على بطاقة رسوميات حقيقية أو افتراضية بدلًا من المعالج.",
  "The Linux personality":
    "شخصية Linux",
  "A second system call layer on the same core. Linux programs run beside Windows ones and share the same files, scheduler and screen.":
    "طبقة استدعاءات نظام ثانية على اللبّ نفسه. تعمل برامج Linux بجانب برامج Windows وتتشارك معها الملفات والمُجدول والشاشة.",
  "Static and dynamically linked (glibc) programs: fork and exec, signals, threads and futexes, epoll, TCP and Unix sockets, shared memory and memfd. Each test's output must match real Linux.":
    "البرامج المربوطة ساكنًا وديناميكيًا (glibc): fork وexec، والإشارات، والخيوط وfutex، وepoll، ومقابس TCP وUnix، والذاكرة المشتركة وmemfd. يجب أن يطابق ناتج كل اختبار نظام Linux الحقيقي.",
  "Job control (stopping and continuing, process groups), vfork, ptys, /proc, and Linux's hard and symbolic links seen from Windows programs.":
    "التحكم في المهام (الإيقاف والاستئناف، ومجموعات العمليات)، وvfork، والطرفيات الزائفة، و/proc، وروابط Linux الصلبة والرمزية كما تراها برامج Windows.",
  "Built into the kernel. GTK programs such as GIMP and Inkscape open their windows on the same desktop as Windows programs.":
    "مدمج في النواة. تفتح برامج GTK مثل GIMP وInkscape نوافذها على سطح المكتب نفسه الذي تستخدمه برامج Windows.",
  "Older X11 programs run through a rootless Xwayland, with gala as the window manager.":
    "تعمل برامج X11 الأقدم عبر Xwayland بلا جذر، وgala هي مدير النوافذ.",
  "Ubuntu roots in the start menu, wsl.exe, and \\\\wsl.localhost paths visible to every Windows program.":
    "جذور Ubuntu في قائمة ابدأ، وwsl.exe، ومسارات \\\\wsl.localhost مرئية لكل برامج Windows.",
  "shasha, the desktop":
    "shasha، سطح المكتب",
  "shasha (Arabic for “screen”) is gala's graphical shell. Everything it does can also be done with the gala command, and a server never starts it.":
    "shasha (بالعربية «شاشة») هي الواجهة الرسومية لـ gala. كل ما تفعله يمكن فعله أيضًا بالأمر gala، ولا يشغّلها الخادم أبدًا.",
  "Windows, a taskbar, a start menu with search, snapping, wallpapers, UI Automation for testers and screen readers.":
    "النوافذ، وشريط المهام، وقائمة ابدأ مع البحث، والإلصاق، وخلفيات الشاشة، وUI Automation للمختبرين وقارئات الشاشة.",
  "One list for Windows and Linux processes alike, Linux ones grouped by distribution, with CPU, memory, performance graphs and “Not responding”, and an End task button that works on either kind.":
    "قائمة واحدة لعمليات Windows وLinux على حدّ سواء، تُجمَّع عمليات Linux حسب التوزيعة، مع المعالج والذاكرة ورسوم الأداء و«لا يستجيب»، وزر «إنهاء المهمة» يعمل على النوعين.",
  "A process's threads with their stacks, its open handles, and a dump file, from the task manager.":
    "خيوط العملية مع مكدساتها، ومقابضها المفتوحة، وملف تفريغ، من مدير المهام.",
  "One tree for the Windows drives and each Linux distribution, with search, copy, paste, rename, delete, properties with Linux permissions, and folders that update live.":
    "شجرة واحدة لمحركات أقراص Windows ولكل توزيعة Linux، مع البحث والنسخ واللصق وإعادة التسمية والحذف، والخصائص مع أذونات Linux، ومجلدات تتحدّث مباشرة.",
  "The Desktop folder and the Recycle Bin, context menus, the Open and Save dialogs programs use, drag and drop with progress, and thumbnails.":
    "مجلد سطح المكتب وسلة المحذوفات، وقوائم السياق، ومربعا حوار الفتح والحفظ اللذان تستخدمهما البرامج، والسحب والإفلات مع شريط التقدّم، والصور المصغّرة.",
  "gala's conhost.exe and cmd.exe, console windows on the desktop, and pseudo consoles for SSH logins.":
    "conhost.exe وcmd.exe الخاصان بـ gala، ونوافذ وحدة التحكم على سطح المكتب، ووحدات تحكم زائفة لتسجيلات الدخول عبر SSH.",
  "Services in session 0 and users in their own, as on Windows: each session with its own desktop, window stations and named objects, SSH logins, and the screen switched between sessions.":
    "الخدمات في الجلسة 0 والمستخدمون في جلساتهم الخاصة، كما في Windows: لكل جلسة سطح مكتبها ومحطات نوافذها وكائناتها المسمّاة، وتسجيلات الدخول عبر SSH، وتبديل الشاشة بين الجلسات.",
  "A session's desktop over RDP, so Windows' own Remote Desktop clients connect to it, a session manager, and sessions carried across a handover.":
    "سطح مكتب الجلسة عبر RDP، فتتصل به عملاء «سطح المكتب البعيد» من Windows نفسها، ومدير جلسات، وجلسات تنتقل عبر التسليم.",
  "In progress now":
    "قيد العمل الآن",
  "The two large efforts under way. Filled dots are done, a ringed dot is being worked on, and hollow dots come later.":
    "الجهدان الكبيران الجاريان. النقاط الممتلئة مكتملة، والنقطة ذات الحلقة قيد العمل، والنقاط المجوّفة تأتي لاحقًا.",
  "Microsoft Office":
    "Microsoft Office",
  "Word, Excel and PowerPoint as Microsoft ships them through Click-to-Run, then the new Outlook. Before any work started, every DLL and function the three apps import was surveyed and ranked against what gala has. That survey sets the order of the work.":
    "Word وExcel وPowerPoint كما توزّعها Microsoft عبر Click-to-Run، ثم Outlook الجديد. قبل بدء أي عمل، حُصرت كل مكتبة DLL ودالة تستوردها التطبيقات الثلاثة ورُتّبت مقابل ما تملكه gala. هذا الحصر يحدّد ترتيب العمل.",
  "Survey what Office imports":
    "حصر ما يستورده Office",
  "1486 files, ranked against gala":
    "1486 ملفًا، مرتّبة مقابل gala",
  "Install with Microsoft's own installer":
    "التثبيت بمثبّت Microsoft نفسه",
  "Click-to-Run onto gala's disk":
    "Click-to-Run على قرص gala",
  "Word and Excel start":
    "Word وExcel يبدآن",
  "their processes load and run":
    "عملياتهما تُحمَّل وتعمل",
  "Word and Excel windows":
    "نوافذ Word وExcel",
  "Word puts its window up, then stops for want of a rendering device":
    "يعرض Word نافذته، ثم يتوقف لغياب جهاز للتصيير",
  "Shapes and images":
    "الأشكال والصور",
  "Direct2D and WIC in software":
    "Direct2D وWIC برمجيًا",
  "Text in Direct2D":
    "النص في Direct2D",
  "through DirectWrite":
    "عبر DirectWrite",
  "Direct3D 11 in software":
    "Direct3D 11 برمجيًا",
  "DXVK over Mesa's lavapipe presents to a window; shaders next":
    "DXVK فوق lavapipe من Mesa يعرض داخل نافذة؛ المظلّلات تاليًا",
  "The new Outlook":
    "Outlook الجديد",
  "WebView2 and MSIX packages":
    "WebView2 وحزم MSIX",
  "ARM64":
    "ARM64",
  "Moving gala to a second processor architecture. Because everything processor-specific already sat behind one seam, this is a port and not a rewrite. The aarch64 kernel now boots to the end on QEMU's virt machine.":
    "نقل gala إلى معمارية معالج ثانية. ولأن كل ما يخص المعالج كان معزولًا أصلًا خلف واجهة واحدة، فهذا نقلٌ لا إعادة كتابة. باتت نواة aarch64 تُقلع حتى النهاية على آلة virt في QEMU.",
  "Boot, page tables, exceptions":
    "الإقلاع وجداول الصفحات والاستثناءات",
  "its own tables, faults reported":
    "جداولها الخاصة، والأعطال تُبلَّغ",
  "Interrupts, timer, threads":
    "المقاطعات والمؤقّت والخيوط",
  "GICv3, the generic timer, scheduler":
    "GICv3 والمؤقّت العام والمُجدول",
  "The second CPU":
    "المعالج الثاني",
  "IPIs and TLB flushes":
    "IPI ومسح TLB",
  "PCI, disk and network":
    "PCI والقرص والشبكة",
  "virtio over ECAM, both ways":
    "virtio عبر ECAM، في الاتجاهين",
  "The clock":
    "الساعة",
  "checked against the PL031 RTC":
    "مُتحقَّق منها مقابل PL031 RTC",
  "User mode":
    "وضع المستخدم",
  "156 x64 test programs and 40 Linux ones pass as ARM64 builds":
    "156 برنامج اختبار x64 و40 برنامجًا لـ Linux تنجح كبُنى ARM64",
  "x64 programs on ARM":
    "برامج x64 على ARM",
  "binary translation":
    "الترجمة الثنائية",
  "What runs today":
    "ما يعمل اليوم",
  "Each program below is held to a boot scenario, named under it. The test harness boots gala in a virtual machine, runs the program and checks its output against what real Windows (or real Linux) gives.":
    "كل برنامج أدناه مقيّد بسيناريو إقلاع مذكور تحته. تُقلع أداة الاختبار gala في آلة افتراضية، وتشغّل البرنامج وتقارن مخرجاته بما يعطيه Windows الحقيقي (أو Linux الحقيقي).",
  "Windows programs, unmodified":
    "برامج Windows، دون تعديل",
  "an unmodified build opens its window, takes input, exposes its accessibility tree and answers ChromeDriver":
    "نسخة غير معدّلة تفتح نافذتها، وتستقبل الإدخال، وتكشف شجرة إمكانية الوصول، وتجيب ChromeDriver",
  "the HotSpot VM compiles and runs a program, and serves HTTP":
    "آلة HotSpot الافتراضية تترجم برنامجًا وتشغّله، وتخدم HTTP",
  "C# on the JIT and the garbage collector":
    "C# على JIT ومجمّع المهملات",
  "pip installs over HTTPS; asyncio, multiprocessing and numpy":
    "pip يثبّت عبر HTTPS؛ وasyncio وmultiprocessing وnumpy",
  "initdb, then the server answering the host's psql":
    "initdb، ثم الخادم يجيب psql من المضيف",
  "key and password logins, an interactive shell, sftp":
    "تسجيل الدخول بالمفتاح وكلمة المرور، وصدفة تفاعلية، وsftp",
  "master and worker processes, HTTPS with OpenSSL":
    "عملية رئيسية وعمليات عاملة، وHTTPS مع OpenSSL",
  "goroutines, garbage collection, child processes, net/http":
    "goroutines وجمع المهملات والعمليات الفرعية وnet/http",
  "runs a command and prints its version table":
    "يشغّل أمرًا ويطبع جدول إصداره",
  "compiles C on gala, then compiles itself":
    "يترجم C على gala، ثم يترجم نفسه",
  "compiled on gala; two processes writing one database":
    "مُترجَم على gala؛ عمليتان تكتبان في قاعدة بيانات واحدة",
  "built on gala by its own makefile":
    "مبنيّ على gala بملف makefile الخاص به",
  "serves a site to the host":
    "يخدم موقعًا للمضيف",
  "makes its certificates and serves FTP over TLS":
    "يُنشئ شهاداته ويخدم FTP عبر TLS",
  "drives another program's window through UI Automation":
    "يتحكّم في نافذة برنامج آخر عبر UI Automation",
  "shell scripts, file tools, make":
    "سكربتات الصدفة وأدوات الملفات وmake",
  "Linux programs":
    "برامج Linux",
  "opens an image, inverts it, exports a file identical to the one real Linux writes":
    "يفتح صورة ويعكس ألوانها ويصدّر ملفًا مطابقًا لما يكتبه Linux الحقيقي",
  "draws an SVG on its canvas and renders it to the same PNG as real Linux":
    "يرسم SVG على لوحته ويحوّله إلى PNG مطابق لما يعطيه Linux الحقيقي",
  "windows drawn and clicked, under Wayland and under X11":
    "نوافذ تُرسم ويُنقر عليها، تحت Wayland وتحت X11",
  "an X11 terminal on a pseudo terminal, through Xwayland":
    "طرفية X11 على طرفية زائفة، عبر Xwayland",
  "the shell and its tools; httpd serving wget":
    "الصدفة وأدواتها؛ وhttpd يخدم wget",
  "On the screen":
    "على الشاشة",
  "The start menu":
    "قائمة ابدأ",
  "Pinned apps install on first click; Linux distributions sit beside them.":
    "التطبيقات المثبّتة في القائمة تُنصَّب عند أول نقرة، وتوزيعات Linux بجانبها.",
  "GIMP":
    "GIMP",
  "A Linux program, through gala's Wayland server, in a desktop window.":
    "برنامج Linux، عبر خادم Wayland في gala، في نافذة على سطح المكتب.",
  "Inkscape":
    "Inkscape",
  "Ubuntu's Inkscape, drawing an SVG on its canvas.":
    "Inkscape من Ubuntu يرسم SVG على لوحته.",
  "Task manager":
    "مدير المهام",
  "Windows apps, background processes and a Linux distribution's processes in one list.":
    "تطبيقات Windows وعمليات الخلفية وعمليات توزيعة Linux في قائمة واحدة.",
  "Performance":
    "الأداء",
  "Both processors, memory, disk and network.":
    "المعالجان والذاكرة والقرص والشبكة.",
  "File explorer":
    "مستكشف الملفات",
  "Browsing a Linux distribution, with the file's Linux permissions.":
    "تصفّح توزيعة Linux، مع صلاحيات Linux للملف.",
  "Xwayland":
    "Xwayland",
  "An X11 xterm, with gala as its window manager.":
    "xterm من X11، وgala مدير نوافذه.",
  "Booting":
    "الإقلاع",
  "قلعة: gala's boot screen.":
    "قلعة: شاشة إقلاع gala.",
  "Planned":
    "مُخطّط",
  "after Chromium, the stock Edge, and the WebView2 runtime that the new Outlook and many other apps embed":
    "بعد Chromium، متصفح Edge الأصلي، وبيئة تشغيل WebView2 التي يضمّنها Outlook الجديد وتطبيقات كثيرة أخرى",
  "gala's desktop over RDP, reached with the Remote Desktop clients people already have (mstsc, the Windows App, FreeRDP), on the sessions that already separate users":
    "سطح مكتب gala عبر RDP، بعملاء سطح المكتب البعيد المتوفرين لدى الناس (mstsc وWindows App وFreeRDP)، على الجلسات التي تفصل المستخدمين أصلًا",
  "Podman, Docker and Kubernetes in a Linux distribution on gala, used from Windows through Docker's own CLI and Podman for Windows, with no virtual machine":
    "Podman وDocker وKubernetes في توزيعة Linux على gala، تُستخدم من Windows عبر واجهة Docker السطرية نفسها وPodman for Windows، دون آلة افتراضية",
  "x64 code translated to ARM64 on the fly, then 32-bit x86 the same way":
    "شيفرة x64 تُترجم إلى ARM64 أثناء التشغيل، ثم x86 بـ 32 بت بالطريقة نفسها",
  "binder, the Android runtime and software graphics, x86_64 images first":
    "binder وبيئة تشغيل Android والرسوميات البرمجية، بدءًا بصور x86_64",
  "every system action described once, so AI clients can use it as a tool and the start menu can take requests in plain language":
    "كل إجراء في النظام موصوف مرة واحدة، فيستخدمه عملاء الذكاء الاصطناعي أداةً، وتتلقى قائمة ابدأ الطلبات بلغة عادية",
  "virtio-sound with WASAPI on top; xHCI with keyboards, mice and storage":
    "virtio-sound وفوقه WASAPI؛ وxHCI مع لوحات المفاتيح والفئران والتخزين",
  "boot the USB image to the desktop and install gala onto the machine's disk, which then boots it by itself":
    "إقلاع صورة USB حتى سطح المكتب وتثبيت gala على قرص الجهاز، الذي يُقلعه بعدها بنفسه",
  "disk, USB, network and power drivers, so a laptop runs gala and takes every update by itself":
    "مُشغّلات القرص وUSB والشبكة والطاقة، ليشغّل الحاسوب المحمول gala ويتلقى كل تحديث بنفسه",
  "QEMU running inside gala, then gala as the hypervisor behind Linux's /dev/kvm and the Windows Hypervisor Platform":
    "QEMU يعمل داخل gala، ثم gala بوصفه المشرف الافتراضي خلف /dev/kvm في Linux وWindows Hypervisor Platform",
  "Building an operating system is mostly a matter of finding out why something went wrong. Much of gala's code exists to make that fast: one command drives everything, and every failure leaves an explanation behind.":
    "بناء نظام تشغيل هو في معظمه مسألة اكتشاف سبب الخلل. وكثير من شيفرة gala موجود لتسريع ذلك: أمر واحد يدير كل شيء، وكل إخفاق يترك تفسيرًا وراءه.",
  "The loop every change follows":
    "الحلقة التي يتبعها كل تغيير",
  "Build and run":
    "البناء والتشغيل",
  "One driver, cargo xtask, builds the whole system and boots it in a pinned QEMU.":
    "أداة واحدة، cargo xtask، تبني النظام كله وتُقلعه في QEMU بإصدار مثبّت.",
  "the kernel, the DLLs, the test programs and a bootable ISO":
    "النواة ومكتبات DLL وبرامج الاختبار وصورة ISO قابلة للإقلاع",
  "boot it in QEMU with the serial log in the terminal":
    "إقلاعه في QEMU مع سجل الأحداث التسلسلي في الطرفية",
  "an image that runs your own program; write it to a USB stick to boot a PC":
    "صورة تشغّل برنامجك الخاص؛ اكتبها على ذاكرة USB لإقلاع حاسوب",
  "keep a server scenario running and open it in your browser, or ssh in":
    "إبقاء سيناريو خادم قيد التشغيل وفتحه في متصفحك، أو الدخول عبر ssh",
  "build the pinned QEMU once":
    "بناء QEMU المثبّت مرة واحدة",
  "Test":
    "الاختبار",
  "Every feature starts as a test program whose expected output is real Windows'. A scenario boots gala, runs it and checks the output.":
    "تبدأ كل ميزة ببرنامج اختبار مخرجاته المتوقعة هي مخرجات Windows الحقيقي. يُقلع السيناريو gala، ويشغّله ويتحقق من المخرجات.",
  "the whole suite: unit tests, gala-um, record and replay, then every boot scenario":
    "المجموعة كاملة: اختبارات الوحدات وgala-um والتسجيل والإعادة، ثم كل سيناريو إقلاع",
  "only the scenarios whose name matches":
    "السيناريوهات التي يطابق اسمها فقط",
  "the test programs in gala-um, in seconds rather than minutes":
    "برامج الاختبار في gala-um، في ثوانٍ بدل دقائق",
  "every test program on gala, checked against the same run on Windows 11":
    "كل برنامج اختبار على gala، مقارنًا بالتشغيل نفسه على Windows 11",
  "the Linux test programs against transcripts from real Linux":
    "برامج اختبار Linux مقارنةً بنصوص مسجّلة من Linux الحقيقي",
  "Find out why":
    "اكتشاف السبب",
  "A failure has to explain itself. Every run ends with a ledger of missing functions, and a failed run leaves its log, a triage summary and the state behind.":
    "على الإخفاق أن يفسّر نفسه. ينتهي كل تشغيل بسجلّ للدوال الناقصة، ويترك التشغيل الفاشل سجل أحداثه وملخص فرز والحالة وراءه.",
  "the summary of a failed run: where it stopped and why":
    "ملخص تشغيل فاشل: أين توقف ولماذا",
  "kernel backtraces resolved to functions":
    "تتبّعات مكدّس النواة محلولةً إلى دوال",
  "PNGs of the screen at chosen moments of a run":
    "صور PNG للشاشة في لحظات مختارة من التشغيل",
  "the JSON event stream, the sampling profiler, and a timeline for ui.perfetto.dev":
    "تدفّق الأحداث بصيغة JSON، والمحلّل بأخذ العينات، وخط زمني لـ ui.perfetto.dev",
  "the same events as OpenTelemetry":
    "الأحداث نفسها بصيغة OpenTelemetry",
  "the files a run wrote to C:, read with the kernel's own code":
    "الملفات التي كتبها التشغيل على C:، مقروءةً بشيفرة النواة نفسها",
  "Shake out the rare bugs":
    "اصطياد الأخطاء النادرة",
  "Races and lost wake-ups are hunted on purpose rather than waited for.":
    "تُطارَد حالات السباق والإيقاظ الضائع عمدًا بدل انتظارها.",
  "programs run while allocations and network sends fail on purpose":
    "برامج تعمل بينما تفشل عمليات التخصيص والإرسال الشبكي عمدًا",
  "deterministic simulation: a race found once is replayed from its seed":
    "محاكاة حتمية: سباق اكتُشف مرة يُعاد تشغيله من بذرته",
  "record a run's inputs and play them back":
    "تسجيل مدخلات التشغيل وإعادة تشغيلها",
  "finds allocations that could panic in the kernel's machine code":
    "يعثر على عمليات تخصيص قد تسبّب panic في شيفرة الآلة للنواة",
  "micro-benchmarks, compared with Windows operation by operation":
    "اختبارات أداء دقيقة، مقارنةً بـ Windows عملية بعملية",
  "Long runs":
    "التشغيلات الطويلة",
  "Some failures come half an hour into a run, as with the Office install. Work on them starts from a saved machine instead of a fresh boot.":
    "بعض الإخفاقات تأتي بعد نصف ساعة من التشغيل، كما في تثبيت Office. ويبدأ العمل عليها من آلة محفوظة بدل إقلاع جديد.",
  "save the whole machine when a log line appears":
    "حفظ الآلة كاملة عند ظهور سطر في سجل الأحداث",
  "resume it, as often as you like":
    "استئنافها، كلما شئت",
  "patch changed DLLs or kernel functions into the resumed machine. Never counted as a pass":
    "ترقيع مكتبات DLL أو دوال النواة المعدّلة في الآلة المستأنفة. لا يُحتسب نجاحًا أبدًا",
  "Beside the code":
    "بجانب الشيفرة",
  "the NT layer built as an ordinary Linux process. Windows test programs run in milliseconds, under gdb, perf and sanitizers, with no virtual machine.":
    "طبقة NT مبنية كعملية Linux عادية. تعمل برامج اختبار Windows في أجزاء من الثانية، تحت gdb وperf وأدوات التعقيم، دون آلة افتراضية.",
  "cargo xtask lint before every commit, and CI's Clippy checks before every push, so a problem is caught before it reaches anyone else.":
    "cargo xtask lint قبل كل commit، وفحوص Clippy في CI قبل كل push، فتُكتشف المشكلة قبل أن تصل إلى أي أحد.",
  "Python helpers: the import survey (what a program needs that gala lacks), the Office survey, the Linux roots built from Ubuntu's packages.":
    "أدوات Python مساعدة: مسح الاستيرادات (ما يحتاجه برنامج ويفتقر إليه gala)، ومسح Office، وجذور Linux المبنية من حزم Ubuntu.",
  "runs every test program on a real Windows 11 machine and writes the report that cargo xtask compare reads.":
    "يشغّل كل برنامج اختبار على جهاز Windows 11 حقيقي ويكتب التقرير الذي يقرؤه cargo xtask compare.",
  "the whole suite in a container, for machines that are not Linux.":
    "المجموعة كاملة في حاوية، للأجهزة التي لا تعمل بـ Linux.",
  "every pull request boots every scenario in QEMU and runs the test programs on a Windows runner.":
    "كل طلب سحب يُقلع كل سيناريو في QEMU ويشغّل برامج الاختبار على جهاز اختبار يعمل بـ Windows.",
  "Inside gala":
    "داخل gala",
  "the system's command line: start, list, snap and close windows, set the wallpaper, list and end processes. It reaches everything the desktop does.":
    "سطر أوامر النظام: فتح النوافذ وسردها وترتيبها وإغلاقها، وتعيين الخلفية، وسرد العمليات وإنهاؤها. يصل إلى كل ما يصل إليه سطح المكتب.",
  "installs software from winget's manifests with the vendor's own installer (zip, NSIS, Inno Setup), and upgrades and uninstalls it.":
    "يثبّت البرامج من بيانات winget بمثبّت المورّد نفسه (zip وNSIS وInno Setup)، ويرقّيها ويزيلها.",
  "the task manager and the file explorer, for Windows and Linux alike.":
    "مدير المهام ومستكشف الملفات، لـ Windows وLinux على السواء.",
  "the command prompt, the console host behind every console window and SSH session, and the door into the Linux distributions.":
    "موجّه الأوامر، ومضيف وحدة التحكم خلف كل نافذة وحدة تحكم وجلسة SSH، والباب إلى توزيعات Linux.",
  "Bootable images of the latest build, for a virtual machine or a PC. They are early developer images: they show the kernel booting and running programs, not a desktop to use yet.":
    "صور قابلة للإقلاع من أحدث بناء، لآلة افتراضية أو لحاسوب شخصي. إنها صور مبكرة للمطوّرين: تُظهر النواة وهي تُقلع وتشغّل البرامج، وليست بعدُ سطح مكتب للاستخدام.",
  "about 55 MB":
    "نحو 55 ميغابايت",
  "For PCs and x86_64 virtual machines. A hybrid image: it boots by BIOS or UEFI, from a CD drive or written to a USB stick.":
    "للحواسيب الشخصية والآلات الافتراضية x86_64. صورة هجينة: تُقلع عبر BIOS أو UEFI، من محرك أقراص مدمجة أو بعد كتابتها على ذاكرة USB.",
  "QEMU (the qemu-system-x86 package, or qemu from Homebrew).":
    "QEMU (الحزمة qemu-system-x86، أو qemu من Homebrew).",
  "about 22 MB":
    "نحو 22 ميغابايت",
  "For QEMU's virt machine on ARM64, booted by UEFI. Real ARM64 hardware is not supported yet.":
    "لآلة virt في QEMU على ARM64، بإقلاع عبر UEFI. عتاد ARM64 الحقيقي غير مدعوم بعد.",
  "QEMU and its ARM64 UEFI firmware: QEMU_EFI.fd from the qemu-efi-aarch64 package (in /usr/share/qemu-efi-aarch64/), or edk2-aarch64-code.fd, which comes with QEMU (Homebrew: $(brew --prefix)/share/qemu/).":
    "QEMU والبرنامج الثابت UEFI الخاص به لـ ARM64: الملف QEMU_EFI.fd من الحزمة qemu-efi-aarch64 (في /usr/share/qemu-efi-aarch64/)، أو edk2-aarch64-code.fd الذي يأتي مع QEMU (في Homebrew: $(brew --prefix)/share/qemu/).",
  "Run it in QEMU":
    "شغّلها في QEMU",
  "Needs":
    "المتطلبات",
  "What you will see":
    "ما الذي ستراه",
  "Each image is a developer build of main's latest commit that passed every test, rebuilt nightly. It boots the kernel, assembles C:, and runs a few demonstration programs: a minimal Windows program on both machines, and on x86_64 also a Go program and one that starts a thousand goroutines across both processors. Their output and the kernel's log go to the serial console, which the command's serial option shows in your terminal: expect lines such as “Hello from user mode!” and “gala: boot ok”. Then QEMU's window shows gala's desktop: the taskbar, the clock, and the start menu, which the Windows key or a click on the start button opens. Close QEMU's window, or press Ctrl+C in the terminal, to stop it.":
    "كل صورة بناءٌ للمطوّرين من أحدث إيداع في main اجتاز جميع الاختبارات، ويُعاد بناؤها كل ليلة. تُقلع النواة، وتُجمّع C:، وتشغّل بضعة برامج توضيحية: برنامج Windows بسيطًا على كلتا الآلتين، وعلى x86_64 أيضًا برنامجًا بلغة Go وآخر يُطلق ألف goroutine موزّعة على المعالجين. يذهب خرجها وسجل أحداث النواة إلى الطرفية التسلسلية، التي يعرضها خيار serial في الأمر داخل طرفيتك: توقّع أسطرًا مثل “Hello from user mode!” و“gala: boot ok”. ثم تعرض نافذة QEMU سطح مكتب gala: شريط المهام والساعة وقائمة ابدأ، التي يفتحها مفتاح Windows أو نقرة على زر ابدأ. أغلق نافذة QEMU، أو اضغط Ctrl+C في الطرفية، لإيقافها.",
  "Check the download":
    "تحقّق من التنزيل",
  "Each image has its SHA-256 beside it. In the folder you downloaded both files to:":
    "لكل صورة قيمة SHA-256 بجانبها. في المجلد الذي نزّلت إليه الملفين:",
  "On a real PC":
    "على حاسوب حقيقي",
  "Principles":
    "المبادئ",
  "The rules every contribution is held to.":
    "القواعد التي تلتزم بها كل مساهمة.",
  "Clean room, without exceptions":
    "غرفة نظيفة، بلا استثناءات",
  "Nothing comes from disassembly, decompiled code or leaked source. The sources are public documentation, the Windows Internals books, mingw-w64's headers, Wine and ReactOS, and how real Windows behaves when tested from the outside.":
    "لا شيء مأخوذ من التفكيك أو الشيفرة المُفكّكة أو الشيفرة المسرّبة. المصادر هي التوثيق العام، وكتب Windows Internals، وترويسات mingw-w64، وWine وReactOS، وسلوك Windows الحقيقي عند اختباره من الخارج.",
  "Compatible at the ABI and nowhere else":
    "متوافق على مستوى ABI ولا شيء غيره",
  "Programs see Windows. Under that boundary, memory, scheduling, I/O and drivers are gala's own, so they can be redesigned and tuned.":
    "ترى البرامج Windows. وتحت هذا الحد، الذاكرة والجدولة والإدخال والإخراج والمُشغّلات من تصميم gala الخاص، فيمكن إعادة تصميمها وضبطها.",
  "Tests first, against real Windows":
    "الاختبارات أولًا، مقابل Windows الحقيقي",
  "A piece of work starts as a Windows program whose expected output is Windows 11's. It is seen failing on gala first, and the implementation follows.":
    "يبدأ كل عمل ببرنامج Windows خرجه المتوقع هو خرج Windows 11. يُرى فاشلًا على gala أولًا، ثم يأتي التنفيذ.",
  "The kernel survives running out of memory":
    "النواة تصمد عند نفاد الذاكرة",
  "Every allocation in the kernel can fail. The APIs that would panic instead are banned by lint.":
    "كل عملية حجز للذاكرة في النواة قابلة للفشل. والواجهات البرمجية التي كانت ستُحدث panic بدلًا من ذلك محظورة عبر lint.",
  "unsafe confined to a short list":
    "unsafe محصور في قائمة قصيرة",
  "Only the architecture layer, boot, the heap and the context switch may use unsafe Rust, and every block states why it is sound.":
    "لا يجوز استخدام unsafe في Rust إلا لطبقة المعمارية والإقلاع والكومة وتبديل السياق، وكل كتلة تذكر سبب سلامتها.",
  "Live update designed in":
    "التحديث الحي جزء من التصميم",
  "State held as versioned data, system calls that can restart, drivers that survive a reset, one clock. These keep a future update without a reboot possible.":
    "حالة محفوظة كبيانات ذات إصدارات، واستدعاءات نظام قابلة للاستئناف، ومُشغّلات تصمد بعد إعادة التعيين، وساعة واحدة. هذه تُبقي التحديث مستقبلًا دون إعادة إقلاع ممكنًا.",
  "A failure explains itself":
    "الفشل يشرح نفسه",
  "Every change ships with the logs, events and saved state that would explain its own failure.":
    "كل تغيير يأتي مع سجلات الأحداث والأحداث والحالة المحفوظة التي تشرح فشله.",
  "Root causes, not wider timeouts":
    "الأسباب الجذرية، لا مهلات أطول",
  "A failure is fixed where it arises. A stopgap is named as one, tracked and replaced.":
    "يُصلَح الفشل حيث ينشأ. والحل المؤقت يُسمّى كذلك، ويُتابَع ويُستبدَل.",
  "Links":
    "روابط",
  "The layers of gala, from programs down to the machine":
    "طبقات gala، من البرامج نزولًا إلى الآلة",
  "Programs, unchanged":
    "برامج، دون تعديل",
  "Windows programs":
    "برامج Windows",
  "Libraries in user mode":
    "مكتبات في وضع المستخدم",
  "gala's clean-room DLLs":
    "مكتبات DLL من gala مكتوبة بأسلوب الغرفة النظيفة",
  "The distribution's own":
    "مكتبات التوزيعة نفسها",
  "The ABI boundary: system calls":
    "حدّ ABI: استدعاءات النظام",
  "the only place where gala imitates anything":
    "المكان الوحيد الذي يحاكي فيه gala أي شيء",
  "Personalities":
    "الشخصيات",
  "NT layer":
    "طبقة NT",
  "handles · processes · registry · security · sockets":
    "المقابض · العمليات · السجل · الأمان · المقابس",
  "Linux layer":
    "طبقة Linux",
  "system calls · signals · epoll · Wayland server":
    "استدعاءات النظام · الإشارات · epoll · خادم Wayland",
  "Shared services":
    "خدمات مشتركة",
  "windows on the screen":
    "النوافذ على الشاشة",
  "Files":
    "الملفات",
  "C: and the disk log":
    "C: وسجل أحداث القرص",
  "Network":
    "الشبكة",
  "Sessions":
    "الجلسات",
  "users and desktops":
    "المستخدمون وأسطح المكتب",
  "The core":
    "اللبّ",
  "Memory · threads and scheduler · one clock · drivers · handover":
    "الذاكرة · الخيوط والمُجدول · ساعة واحدة · المُشغّلات · التسليم",
  "The architecture seam":
    "فاصل المعمارية",
  "works":
    "يعمل",
  "in progress":
    "قيد العمل",
  "The machine":
    "الآلة",
  "A PC or a virtual machine: UEFI or BIOS · virtio disk, network and display · PS/2":
    "حاسوب شخصي أو آلة افتراضية: UEFI أو BIOS · قرص وشبكة وعرض عبر virtio · PS/2",
  "Windows programs are on the left and Linux programs on the right. Both go down to the same core. Everything below the boundary is gala's own design.":
    "برامج Windows على اليمين وبرامج Linux على اليسار. كلاهما ينزل إلى اللبّ نفسه. وكل ما تحت الحد من تصميم gala الخاص.",
  "user mode":
    "وضع المستخدم",
  "kernel":
    "النواة",
  "How Windows and Linux programs get a window on the same desktop":
    "كيف تحصل برامج Windows وLinux على نافذة في سطح المكتب نفسه",
  "system calls":
    "استدعاءات النظام",
  "a Unix socket to gala's Wayland server":
    "مقبس Unix إلى خادم Wayland في gala",
  "xterm and other X11 programs":
    "xterm وبرامج X11 الأخرى",
  "the same Wayland server":
    "خادم Wayland نفسه",
  "shasha: one window list, one taskbar, one focus":
    "shasha: قائمة نوافذ واحدة، وشريط مهام واحد، وتركيز واحد",
  "framebuffer or virtio-gpu":
    "framebuffer أو virtio-gpu",
  "Three routes lead to the same window list. The taskbar, the task manager and UI Automation treat every window alike, whichever system it came from.":
    "ثلاثة مسارات تؤدي إلى قائمة النوافذ نفسها. يعامل شريط المهام ومدير المهام وUI Automation كل نافذة بالطريقة نفسها، أيًّا كان النظام الذي جاءت منه.",
  "Works":
    "يعمل",
  "running kernel":
    "النواة العاملة",
  "new kernel":
    "النواة الجديدة",
  "this machine":
    "هذه الآلة",
  "another machine":
    "آلة أخرى",
  "done":
    "مكتمل",
  "planned":
    "مُخطّط",
  "The test-first loop":
    "حلقة الاختبار أولًا",
  "Write a test program":
    "اكتب برنامج اختبار",
  "a small Windows .exe in user/<feature>_test":
    "ملف .exe صغير لـ Windows في user/<feature>_test",
  "Run it on Windows 11":
    "شغّله على Windows 11",
  "its output is the expected result":
    "خرجه هو النتيجة المتوقعة",
  "Run it on gala":
    "شغّله على gala",
  "in gala-um (seconds) and booted in QEMU":
    "في gala-um (ثوانٍ) وبإقلاع في QEMU",
  "See it fail, then implement":
    "شاهده يفشل، ثم نفّذ",
  "until gala's output equals Windows'":
    "حتى يطابق خرج gala خرج Windows",
  "Check it in":
    "أودِعه",
  "the scenario joins the suite CI runs on every change":
    "ينضم السيناريو إلى المجموعة التي يشغّلها CI مع كل تغيير",
  "the same binary runs on both systems, and the two outputs are compared":
    "الملف التنفيذي نفسه يعمل على النظامين، ويُقارَن الخرجان",
  "The x86_64 image boots a PC directly, by BIOS or UEFI. Written to a USB stick, it runs entirely from memory: nothing is installed, and gala has no driver yet for the computer's own disks, so they are left as they were.":
    "تُقلع صورة x86_64 حاسوبًا شخصيًا مباشرة، عبر BIOS أو UEFI. وعند كتابتها على ذاكرة USB تعمل بالكامل من الذاكرة: لا يُثبَّت شيء، وليس لدى gala بعدُ مُشغّل لأقراص الحاسوب نفسه، فتبقى كما كانت.",
  "A USB stick of 1 GB or more whose contents you can lose: writing the image erases it.":
    "ذاكرة USB بسعة 1 غيغابايت أو أكثر يمكنك الاستغناء عن محتواها: كتابة الصورة تمحوها.",
  "A 64-bit Intel or AMD PC with 512 MB of memory or more (Intel from 2012 or AMD from 2015 onwards, for its random-number instruction).":
    "حاسوب شخصي 64 بت بمعالج Intel أو AMD وذاكرة 512 ميغابايت أو أكثر (Intel من 2012 أو AMD من 2015 فصاعدًا، لتعليمة الأرقام العشوائية).",
  "gala-x86_64.iso from the download above, its SHA-256 checked.":
    "الملف gala-x86_64.iso من التنزيل أعلاه، بعد التحقق من SHA-256 الخاص به.",
  "Write the image to the stick":
    "اكتب الصورة على ذاكرة USB",
  "The image is written byte for byte to the whole stick, not copied onto it as a file. Check the stick's name twice: the wrong one erases another disk.":
    "تُكتب الصورة بايتًا ببايت على الذاكرة بأكملها، ولا تُنسخ إليها كملف. تحقّق من اسم الذاكرة مرتين: اختيار الخاطئ يمحو قرصًا آخر.",
  "With Rufus: choose the stick under Device, choose the image with SELECT, then START. When Rufus asks how to write it, choose DD Image mode. balenaEtcher works too: Flash from file, Select target, Flash.":
    "باستخدام Rufus: اختر الذاكرة تحت Device، واختر الصورة عبر SELECT، ثم START. وحين يسألك Rufus عن طريقة الكتابة، اختر وضع DD Image. ويعمل balenaEtcher أيضًا: Flash from file، ثم Select target، ثم Flash.",
  "Find the stick in the list (say /dev/disk4), unmount it, write the image to its raw device (rdisk), then eject it. Or use balenaEtcher.":
    "اعثر على الذاكرة في القائمة (لنقل /dev/disk4)، وألغِ تركيبها، واكتب الصورة على جهازها الخام (rdisk)، ثم أخرجها. أو استخدم balenaEtcher.",
  "Find the stick in the list (say /dev/sdb: the whole device, not a partition such as /dev/sdb1), then write the image to it.":
    "اعثر على الذاكرة في القائمة (لنقل /dev/sdb: الجهاز بأكمله، لا قسمًا مثل /dev/sdb1)، ثم اكتب الصورة عليها.",
  "Turn Secure Boot off":
    "أوقف Secure Boot",
  "On a UEFI PC, open the firmware's settings (a key at power-on, often F2, Del or F10), find Secure Boot under Security or Boot and disable it: the bootloader on the image is not signed yet. If Windows on that PC uses BitLocker, have its recovery key at hand, as changing Secure Boot can make Windows ask for it. Turn Secure Boot back on when you are done.":
    "على حاسوب يعمل بـ UEFI، افتح إعدادات البرنامج الثابت (بمفتاح عند التشغيل، غالبًا F2 أو Del أو F10)، واعثر على Secure Boot ضمن Security أو Boot وعطّله: مُحمّل الإقلاع في الصورة غير موقّع بعد. وإن كان Windows على ذلك الحاسوب يستخدم BitLocker، فاجعل مفتاح الاسترداد في متناولك، إذ قد يجعل تغيير Secure Boot نظام Windows يطلبه. أعِد تفعيل Secure Boot حين تنتهي.",
  "Start the PC from the stick":
    "شغّل الحاسوب من ذاكرة USB",
  "Plug the stick in, power the PC on and press its boot menu key (often F12, F11, F10, F9 or Esc), then choose the USB stick; where it is listed twice, the UEFI entry. On an Intel Mac, hold the Option key at power-on. Macs with Apple silicon cannot start it.":
    "صِل الذاكرة، وشغّل الحاسوب واضغط مفتاح قائمة الإقلاع (غالبًا F12 أو F11 أو F10 أو F9 أو Esc)، ثم اختر ذاكرة USB؛ وإن ظهرت مرتين، فاختر مُدخل UEFI. على حاسوب Mac بمعالج Intel، اضغط مطولًا على مفتاح Option عند التشغيل. أما أجهزة Mac بمعالجات Apple silicon فلا يمكنها تشغيلها.",
  "gala's boot screen, then its log scrolling as it starts, and then its desktop with the taskbar and the start menu. To leave, hold the power button until the PC turns off and take the stick out: the PC then starts its usual system.":
    "شاشة إقلاع gala، ثم سجل أحداثها يتمرّر أثناء بدء التشغيل، ثم سطح مكتبها مع شريط المهام وقائمة ابدأ. للخروج، اضغط مطولًا على زر التشغيل حتى ينطفئ الحاسوب وأخرج الذاكرة: عندها يبدأ الحاسوب نظامه المعتاد.",
  "It is tested in virtual machines, booted from a USB stick under BIOS and UEFI, but not yet on physical PCs: expect some not to start it. What the screen shows when one stops is worth reporting.":
    "إنها مُختبَرة في آلات افتراضية، بإقلاع من ذاكرة USB عبر BIOS وUEFI، لكن ليس بعدُ على حواسيب فعلية: توقّع ألّا يشغّلها بعضها. وما تعرضه الشاشة حين يتوقف أحدها يستحق الإبلاغ عنه.",
  "The keyboard and mouse work where they reach gala as PS/2 devices: a laptop's built-in keyboard often does, and many PCs present USB ones that way while their firmware's USB legacy support is on. gala's own USB driver is planned.":
    "تعمل لوحة المفاتيح والفأرة حين تصلان إلى gala كأجهزة PS/2: وهذا حال لوحة المفاتيح المدمجة في الحواسيب المحمولة غالبًا، وتقدّم حواسيب كثيرة أجهزة USB بهذه الطريقة ما دام دعم USB القديم في برنامجها الثابت مفعّلًا. أما مُشغّل USB الخاص بـ gala فمُخطّط.",
  "There is no network on a physical PC yet, and no sound.":
    "لا شبكة على الحاسوب الفعلي بعد، ولا صوت.",
  "Run it from a USB stick":
    "شغّله من ذاكرة USB",
  "You need":
    "ما تحتاج إليه",
  "Limits for now":
    "القيود حاليًا",
};
