// filezilla_flaui: FileZilla 3.71.0 (wxWidgets 3.2) driven through UI
// Automation with FlaUI (FlaUI.Core and FlaUI.UIA3 5.0.0, MIT), as a
// tester drives it on Windows: no pixels, no coordinates. Two servers,
// by the first argument:
//
// - none (or a path): FTP over TLS. FileZilla Server 1.12.6 already runs
//   (the scenario starts it: FTP on port 7, user `gala`, its home C:\ftp
//   holding hello.txt);
// - `sftp`: SFTP through FileZilla's own SSH engine (libfzssh). Win32-
//   OpenSSH's sshd already runs as a service (port 22, password logons,
//   the sftp subsystem); FileZilla logs on as `galatest` (password
//   Gala-Test-1) and opens C:\ftp (sftp://127.0.0.1/C:/ftp; it holds
//   hello.txt).
//
// The test
//
// 1. starts FileZilla (C:\filezilla\filezilla.exe, or the path given as
//    the last argument) with Application.Launch and finds its main
//    window (GetMainWindow: the process's unowned visible window);
// 2. closes the Welcome dialog: a Window owned by the main window (UIA
//    puts owned windows under their owner), its "OK" button invoked;
// 3. fills the Quickconnect bar's edits, found by the static labels in
//    front of them (UIA's Win32 proxy names an edit after its label:
//    "Host:", "Username:", "Password:", "Port:"; matched as substrings),
//    through the Value pattern, and invokes the Quickconnect button;
// 4. answers the dialogs the first connection brings (whether to
//    remember passwords; the server's unknown, self-signed certificate,
//    or over SFTP its unknown host key) with their OK buttons, as they come, until the remote file list (a
//    virtual list view: wx answers LVN_GETDISPINFO) has an item
//    "hello.txt";
// 5. reads the local file list (the list view left of the remote one);
//    reads the remote list as a grid (a report-view list view: the Grid
//    and Table patterns, its header's columns "Filename", "Filesize",
//    ..., hello.txt's row and its cells);
// 6. selects hello.txt (SelectionItem) and checks that it is selected and
//    the list's Selection names it;
// 7. ends FileZilla through its menu bar: File (ExpandCollapse), then
//    Exit (Invoke), and waits for the process to end.
//
// UI Automation events while it connects: the main window's Name (its
// title, which names the server once connected: a Name property-changed
// event), and the dialogs opening (Window_WindowOpened under the main
// window, which owns them).
//
// Expected output (Windows 11 24H2 and gala; FileZilla's own texts),
// over FTP (over SFTP: quickconnect=sftp://127.0.0.1/C:/ftp galatest 22,
// dialog=Unknown host key OK and opened=...,Unknown host key):
//
//   window=FileZilla
//   welcome=Welcome to FileZilla OK
//   quickconnect=127.0.0.1 gala 7
//   dialog=Remember passwords? OK
//   dialog=Unknown certificate OK
//   remote=..,hello.txt
//   local=<n> ..,<the local directory's entries>
//   grid=Filename,Filesize,Filetype,Last modified,Permissions,Owner/Group rows=2
//   row=hello.txt <its size>
//   events=title opened=Remember passwords?,Unknown certificate
//   selected=hello.txt True
//   menu=File Exit
//   exited=True
//   filezilla_flaui: ok
//
// When an element is not found the test prints the tree below the main
// window (control type, name, automation id, class) to show what there
// was. Built by build.sh with flaui_test.
using System;
using System.Collections.Concurrent;
using System.IO;
using System.Linq;
using FlaUI.Core;
using FlaUI.Core.AutomationElements;
using FlaUI.Core.Conditions;
using FlaUI.Core.Definitions;
using FlaUI.Core.Tools;
using FlaUI.UIA3;

static class FileZillaFlaUI
{
    static readonly TimeSpan Long = TimeSpan.FromSeconds(90);
    static readonly TimeSpan Poll = TimeSpan.FromMilliseconds(200);

    /// What the event handlers saw (on UIA's thread).
    static readonly ConcurrentQueue<string> Events = new ConcurrentQueue<string>();

    static int Fail(int n, string what)
    {
        Console.WriteLine("filezilla_flaui: check " + n + " failed: " + what);
        return n;
    }

    static int Main(string[] args)
    {
        try
        {
            var sftp = args.Length > 0 && args[0] == "sftp";
            var rest = args.Skip(sftp ? 1 : 0).ToArray();
            return Run(rest.Length > 0 ? rest[0] : @"C:\filezilla\filezilla.exe", sftp);
        }
        catch (Exception e)
        {
            Console.WriteLine("filezilla_flaui: exception: " + e);
            return 99;
        }
    }

    /// The tree below `e` (to `depth` levels), one element per line.
    static void Dump(AutomationElement e, int depth, string indent = "")
    {
        try
        {
            Console.WriteLine("filezilla_flaui: tree: " + indent + e.ControlType + " '" + e.Name + "' id=" +
                e.AutomationId + " class=" + e.ClassName);
            if (depth == 0)
                return;
            var n = 0;
            foreach (var c in e.FindAllChildren())
            {
                if (++n > 40)
                {
                    Console.WriteLine("filezilla_flaui: tree: " + indent + "  ...");
                    break;
                }
                Dump(c, depth - 1, indent + "  ");
            }
        }
        catch (Exception x)
        {
            Console.WriteLine("filezilla_flaui: tree: " + indent + "(" + x.GetType().Name + ")");
        }
    }

    /// A dialog owned by the main window (a Window child), or null.
    static Window Dialog(Window main)
    {
        var d = main.FindFirstChild(cf => cf.ByControlType(ControlType.Window));
        return d?.AsWindow();
    }

    /// The dialog's OK button, invoked; its title.
    static string AnswerOk(Window dialog)
    {
        var title = dialog.Title;
        var ok = Retry.WhileNull(() => dialog.FindFirstDescendant(cf =>
            cf.ByControlType(ControlType.Button).And(cf.ByName("OK"))), TimeSpan.FromSeconds(10), Poll).Result;
        if (ok == null)
        {
            Dump(dialog, 3);
            return null;
        }
        ok.AsButton().Invoke();
        // The dialog goes once FileZilla has handled the click.
        Retry.WhileTrue(() => Showing(dialog), TimeSpan.FromSeconds(30), Poll);
        return title;
    }

    /// Whether the element is still there and shown (it may go between
    /// two property reads: ElementNotAvailable then means gone).
    static bool Showing(AutomationElement e)
    {
        try
        {
            return e.IsAvailable && !e.IsOffscreen;
        }
        catch (Exception)
        {
            return false;
        }
    }

    /// The Quickconnect bar's edit labelled `label`.
    static TextBox Field(Window main, string label)
    {
        var e = main.FindFirstDescendant(cf => cf.ByControlType(ControlType.Edit)
            .And(cf.ByName(label, PropertyConditionFlags.MatchSubstring)));
        return e?.AsTextBox();
    }

    static string Texts(ListBox list, int max)
    {
        return string.Join(",", list.Items.Take(max).Select(i => i.Text));
    }

    static int Run(string path, bool sftp)
    {
        // The second dialog of a first connection: the server's identity.
        var identity = sftp ? "Unknown host key" : "Unknown certificate";
        var app = Application.Launch(path);
        try
        {
            using (var automation = new UIA3Automation())
            {
                automation.TransactionTimeout = TimeSpan.FromSeconds(60);
                var window = app.GetMainWindow(automation, Long);
                if (window == null)
                    return Fail(1, "no main window");
                Console.WriteLine("window=" + window.Title);
                if (!window.Title.StartsWith("FileZilla"))
                    return Fail(2, window.Title);

                // The Welcome dialog, owned by the main window.
                var welcome = Retry.WhileNull(() => Dialog(window), Long, Poll).Result;
                if (welcome == null)
                {
                    Dump(window, 2);
                    return Fail(3, "no Welcome dialog");
                }
                var title = welcome.Title;
                var answered = AnswerOk(welcome);
                Console.WriteLine("welcome=" + title + " " + (answered != null ? "OK" : "-"));
                if (answered == null || title != "Welcome to FileZilla")
                    return Fail(4, "welcome");

                // Quickconnect.
                var host = Field(window, "Host");
                var user = Field(window, "Username");
                var password = Field(window, "Password");
                var port = Field(window, "Port");
                if (host == null || user == null || password == null || port == null)
                {
                    Dump(window, 3);
                    return Fail(5, "Quickconnect fields " + (host != null) + (user != null) +
                        (password != null) + (port != null));
                }
                if (!password.Properties.IsPassword.Value)
                    return Fail(6, "password field not a password");
                host.Text = sftp ? "sftp://127.0.0.1/C:/ftp" : "127.0.0.1";
                user.Text = sftp ? "galatest" : "gala";
                password.Text = sftp ? "Gala-Test-1" : "gala";
                port.Text = sftp ? "22" : "7";
                Console.WriteLine("quickconnect=" + host.Text + " " + user.Text + " " + port.Text);
                var connect = window.FindFirstDescendant(cf =>
                    cf.ByControlType(ControlType.Button).And(cf.ByName("Quickconnect")));
                if (connect == null)
                {
                    Dump(window, 3);
                    return Fail(7, "no Quickconnect button");
                }
                // Events while it connects: the title (Name) and the
                // dialogs opening.
                var titles = window.RegisterPropertyChangedEvent(TreeScope.Element,
                    (e, p, v) => Events.Enqueue("title=" + v), automation.PropertyLibrary.Element.Name);
                var opened = window.RegisterAutomationEvent(automation.EventLibrary.Window.WindowOpenedEvent,
                    TreeScope.Descendants, (e, id) =>
                    {
                        try
                        {
                            Events.Enqueue("opened=" + e.Name);
                        }
                        catch (Exception)
                        {
                            // Gone already.
                        }
                    });
                connect.AsButton().Invoke();

                // The dialogs, as they come, until the remote list shows
                // hello.txt.
                ListBox remote = null;
                AutomationElement helloItem = null;
                var deadline = DateTime.Now + TimeSpan.FromSeconds(180);
                var dialogs = 0;
                while (helloItem == null && DateTime.Now < deadline)
                {
                    var dialog = Dialog(window);
                    if (dialog != null)
                    {
                        if (++dialogs > 4)
                            return Fail(8, "too many dialogs");
                        var t = AnswerOk(dialog);
                        Console.WriteLine("dialog=" + (t ?? dialog.Title) + " " + (t != null ? "OK" : "-"));
                        if (t == null)
                            return Fail(9, "dialog without OK");
                        continue;
                    }
                    try
                    {
                        foreach (var l in window.FindAllDescendants(cf => cf.ByClassName("SysListView32")))
                        {
                            var item = l.FindFirstChild(cf =>
                                cf.ByControlType(ControlType.ListItem).And(cf.ByName("hello.txt")));
                            if (item != null)
                            {
                                remote = l.AsListBox();
                                helloItem = item;
                                break;
                            }
                        }
                    }
                    catch (Exception)
                    {
                        // A window went while the tree was read (FileZilla
                        // rebuilds its panes as it connects): look again.
                    }
                    if (helloItem == null)
                        System.Threading.Thread.Sleep(500);
                }
                if (remote == null || helloItem == null)
                {
                    Dump(window, 3);
                    return Fail(10, "no hello.txt in a list");
                }
                Console.WriteLine("remote=" + Texts(remote, 8));

                // The local list: the list view left of the remote one, at
                // the same height.
                var r = remote.BoundingRectangle;
                var local = window.FindAllDescendants(cf => cf.ByClassName("SysListView32"))
                    .Where(l => l.BoundingRectangle.Top == r.Top && l.BoundingRectangle.Right <= r.Left)
                    .Select(l => l.AsListBox()).FirstOrDefault();
                if (local == null)
                    return Fail(11, "no local list");
                var localItems = local.Items;
                Console.WriteLine("local=" + localItems.Length + " " + Texts(local, 8));
                if (localItems.Length == 0 || localItems[0].Text != "..")
                    return Fail(12, "local list " + Texts(local, 8));

                // The remote list as a grid: its columns (the header's
                // items) and hello.txt's row.
                var grid = remote.AsGrid();
                var columns = string.Join(",", grid.ColumnHeaders.Select(h => h.Name));
                Console.WriteLine("grid=" + columns + " rows=" + grid.RowCount);
                if (!columns.StartsWith("Filename,Filesize,") || grid.RowCount != 2 ||
                    grid.ColumnCount != grid.ColumnHeaders.Length)
                    return Fail(18, "grid " + columns + " " + grid.RowCount + "x" + grid.ColumnCount);
                var cells = grid.GetRowByIndex(1).Cells;
                Console.WriteLine("row=" + (cells.Length > 1 ? cells[0].Name + " " + cells[1].Name : "-"));
                if (cells.Length != grid.ColumnCount || cells[0].Name != "hello.txt" || cells[1].Name.Length == 0)
                    return Fail(19, "hello.txt's row");

                // The events seen while connecting.
                var seen = Events.ToArray();
                var titled = seen.Any(e => e.StartsWith("title=") && e.Contains("127.0.0.1"));
                var dialogsOpened = string.Join(",", seen.Where(e => e.StartsWith("opened=")).Select(e => e.Substring(7)));
                Console.WriteLine("events=" + (titled ? "title" : "-") + " opened=" + dialogsOpened);
                if (!titled || !dialogsOpened.Contains("Remember passwords?") || !dialogsOpened.Contains(identity))
                    return Fail(20, "events: " + string.Join(";", seen));
                titles.Dispose();
                opened.Dispose();

                // Select hello.txt.
                var hello = helloItem.AsListBoxItem();
                hello.Select();
                var selected = remote.SelectedItem;
                Console.WriteLine("selected=" + (selected == null ? "-" : selected.Text) + " " + hello.IsSelected);
                if (selected == null || selected.Text != "hello.txt" || !hello.IsSelected)
                    return Fail(13, "selection");

                // File > Exit on the menu bar.
                var bar = window.FindFirstChild(cf => cf.ByControlType(ControlType.MenuBar));
                if (bar == null)
                {
                    Dump(window, 1);
                    return Fail(14, "no menu bar");
                }
                var file = bar.AsMenu().Items.FirstOrDefault(i => i.Text == "File");
                if (file == null)
                {
                    Dump(bar, 1);
                    return Fail(15, "no File menu");
                }
                file.Expand();
                var exit = Retry.WhileNull(() => window.FindFirstDescendant(cf =>
                    cf.ByControlType(ControlType.MenuItem).And(cf.ByName("Exit"))), TimeSpan.FromSeconds(30), Poll).Result;
                if (exit == null)
                {
                    Dump(window, 2);
                    return Fail(16, "no Exit item");
                }
                Console.WriteLine("menu=" + file.Text + " " + exit.Name);
                exit.AsMenuItem().Invoke();
            }
            var exited = Retry.WhileFalse(() => app.HasExited, TimeSpan.FromSeconds(60), Poll).Result;
            Console.WriteLine("exited=" + exited);
            if (!exited)
                return Fail(17, "FileZilla still runs");
        }
        finally
        {
            app.Dispose();
        }
        Console.WriteLine("filezilla_flaui: ok");
        return 0;
    }
}
