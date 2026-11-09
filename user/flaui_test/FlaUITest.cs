// flaui_test: FlaUI (FlaUI.Core and FlaUI.UIA3 5.0.0, MIT) driving another
// program's window through UI Automation, as a screen test would: it
// starts flaui_target.exe (next to this assembly) with
// Application.Launch, gets its main window with GetMainWindow, finds the
// controls by AutomationId (also with a cache request: FindAllBuildCache
// and the Cached property getters), invokes the button and waits for the
// title it sets, types into the edit through the Value pattern and reads it back,
// toggles the check box, reads the list view's items (AsListBox: its
// ListItem children, their Text) and selects one (SelectionItem; the
// list's Selection gives it back), uses FlaUI's Window API (the Window
// pattern's properties, ModalWindows, the TitleBar and its buttons,
// Window.Move through the Transform pattern, SetWindowVisualState
// Maximized and Normal) and closes the program with Window.Close (the
// title bar's Close button: WM_SYSCOMMAND SC_CLOSE, then WM_CLOSE).
//
// The list view in report view is also read as a grid (FlaUI's Grid: the
// Grid and Table patterns; its Header's HeaderItems; GetItem's cells
// with GridItem and TableItem; a row's cells, its children). UI
// Automation events reach FlaUI's handlers (on UIA's thread): the
// target's window opening (Window_WindowOpened on the desktop, Subtree)
// and taking the focus (a focus-changed event from its process), its
// title changing (Name), the edit's value and the check box's state
// changing (Value.Value, Toggle.ToggleState), a list item being selected
// (SelectionItem.ElementSelected) and the window closing
// (Window_WindowClosed).
//
// Expected output on Windows 11 24H2 (build 26100) with .NET 8 (FlaUI's
// documented behaviour; UIA3 runs on CUIAutomation8, whose
// IUIAutomation2 timeouts default to 20 s (transaction) and 2 s
// (connection); UIA's Win32 proxies give a push button ControlType Button
// with the control id as AutomationId and the text as Name, an edit the
// Value pattern, an auto check box the Toggle pattern, a list view's
// items ListItem children with their column-0 text as Name and the
// SelectionItem pattern, the list the Selection pattern; the window's only
// Button child is "Press", the caption buttons being the title bar's):
//
//   timeouts=20000/2000
//   timeout=30000
//   window=FlaUI Target
//   pid=ok
//   opened=FlaUI Target
//   focus=True
//   cached=1 Press 101
//   button=Press Button
//   title=FlaUI Pressed
//   namechange=FlaUI Pressed
//   text=hello
//   valuechange=hello
//   checked=False
//   checked=True
//   togglechange=On
//   items=3 alpha.txt,beta.txt,gamma.txt
//   selected=beta.txt True False
//   selectedevent=beta.txt
//   grid=3x2 RowMajor headers=Name,Size header=Name,Size
//   cell=22 1,1 104 Size
//   row=gamma.txt,333
//   window=True True False False Normal modal=0
//   titlebar=FlaUI Pressed Minimize,Maximize,Close
//   moved=100,80
//   state=Maximized
//   state=Normal 100,80
//   closed=True
//   closedevent=True
//   flaui_test: ok
//
// Built by build.sh (Mono's mcs against the .NET 8 runtime's assemblies
// and FlaUI's; xtask fetches FlaUI). Each check that fails prints what it
// saw and returns its number.
using System;
using System.Collections.Concurrent;
using System.IO;
using System.Linq;
using FlaUI.Core;
using FlaUI.Core.AutomationElements;
using FlaUI.Core.Definitions;
using FlaUI.Core.Tools;
using FlaUI.UIA3;

static class FlaUITest
{
    static int Fail(int n, string what)
    {
        Console.WriteLine("flaui_test: check " + n + " failed: " + what);
        return n;
    }

    static int Main()
    {
        try
        {
            return Run();
        }
        catch (Exception e)
        {
            Console.WriteLine("flaui_test: exception: " + e);
            return 99;
        }
    }

    /// What the event handlers saw (they run on UIA's thread).
    static readonly ConcurrentQueue<string> Events = new ConcurrentQueue<string>();

    /// The first event seen starting with `prefix` (waiting up to 10 s), or null.
    static string Event(string prefix)
    {
        string found = null;
        Retry.WhileFalse(() =>
        {
            found = Events.FirstOrDefault(e => e.StartsWith(prefix));
            return found != null;
        }, TimeSpan.FromSeconds(10), TimeSpan.FromMilliseconds(50));
        return found == null ? null : found.Substring(prefix.Length);
    }

    static string Safe(Func<string> read)
    {
        try
        {
            return read();
        }
        catch (Exception e)
        {
            return "(" + e.GetType().Name + ")";
        }
    }

    static int Run()
    {
        var target = Path.Combine(AppContext.BaseDirectory, "flaui_target.exe");
        using (var automation = new UIA3Automation())
        {
            // Events from before the program starts: its window opening
            // and taking the focus.
            var desktop = automation.GetDesktop();
            var opened = desktop.RegisterAutomationEvent(automation.EventLibrary.Window.WindowOpenedEvent,
                TreeScope.Subtree, (e, id) => Events.Enqueue("opened=" + Safe(() => e.Name)));
            var focus = automation.RegisterFocusChangedEvent(e =>
                Events.Enqueue("focus=" + Safe(() => e.Properties.ProcessId.Value.ToString())));
            var app = Application.Launch(target);
            try
            {
                // CUIAutomation8's IUIAutomation2 (FlaUI throws if it is missing).
                Console.WriteLine("timeouts=" + automation.TransactionTimeout.TotalMilliseconds + "/" +
                    automation.ConnectionTimeout.TotalMilliseconds);
                automation.TransactionTimeout = TimeSpan.FromSeconds(30);
                Console.WriteLine("timeout=" + automation.TransactionTimeout.TotalMilliseconds);
                var window = app.GetMainWindow(automation, TimeSpan.FromSeconds(30));
                if (window == null)
                    return Fail(1, "no main window");
                Console.WriteLine("window=" + window.Title);
                if (window.Title != "FlaUI Target")
                    return Fail(2, window.Title);
                if (window.Properties.ProcessId.Value != app.ProcessId)
                    return Fail(3, "process id " + window.Properties.ProcessId.Value);
                Console.WriteLine("pid=ok");
                var openedName = Event("opened=FlaUI");
                Console.WriteLine("opened=" + (openedName == null ? "-" : "FlaUI" + openedName));
                if (openedName != " Target")
                    return Fail(22, "no WindowOpened event: " + string.Join(";", Events));
                var focused = Event("focus=" + app.ProcessId);
                Console.WriteLine("focus=" + (focused != null));
                if (focused == null)
                    return Fail(23, "no focus-changed event from the program");
                opened.Dispose();
                focus.Dispose();
                var nameEvents = window.RegisterPropertyChangedEvent(TreeScope.Element,
                    (e, p, v) => Events.Enqueue("name=" + v), automation.PropertyLibrary.Element.Name);

                // A cache request: the window's Button children with their
                // Name and AutomationId cached.
                var request = new CacheRequest();
                request.TreeScope = TreeScope.Element;
                request.AutomationElementMode = AutomationElementMode.Full;
                request.Add(automation.PropertyLibrary.Element.Name);
                request.Add(automation.PropertyLibrary.Element.AutomationId);
                using (request.Activate())
                {
                    var buttons = window.FindAllChildren(cf => cf.ByControlType(ControlType.Button));
                    Console.WriteLine("cached=" + buttons.Length + " " +
                        (buttons.Length > 0 ? buttons[0].Name + " " + buttons[0].AutomationId : "-"));
                    if (buttons.Length != 1 || buttons[0].Name != "Press" || buttons[0].AutomationId != "101")
                        return Fail(13, "cached buttons");
                }

                // The button: found by AutomationId (the control id).
                var element = window.FindFirstDescendant(cf => cf.ByAutomationId("101"));
                if (element == null)
                    return Fail(4, "no element 101");
                var button = element.AsButton();
                Console.WriteLine("button=" + button.Name + " " + button.ControlType);
                if (button.Name != "Press" || button.ControlType != ControlType.Button)
                    return Fail(5, "button");
                button.Invoke();
                var pressed = Retry.WhileFalse(() => window.Title == "FlaUI Pressed", TimeSpan.FromSeconds(10),
                    TimeSpan.FromMilliseconds(50));
                Console.WriteLine("title=" + window.Title);
                if (!pressed.Result)
                    return Fail(6, "title not set");
                var nameChange = Event("name=FlaUI Pressed");
                Console.WriteLine("namechange=" + (nameChange == null ? "-" : "FlaUI Pressed"));
                if (nameChange == null)
                    return Fail(24, "no Name change event");
                nameEvents.Dispose();

                // The edit: the Value pattern.
                var edit = window.FindFirstDescendant(cf => cf.ByAutomationId("102"));
                if (edit == null)
                    return Fail(7, "no element 102");
                var textBox = edit.AsTextBox();
                var values = edit.RegisterPropertyChangedEvent(TreeScope.Element,
                    (e, p, v) => Events.Enqueue("value=" + v), automation.PropertyLibrary.Value.Value);
                textBox.Text = "hello";
                Console.WriteLine("text=" + textBox.Text);
                if (textBox.Text != "hello")
                    return Fail(8, "text");
                var valueChange = Event("value=hello");
                Console.WriteLine("valuechange=" + (valueChange == null ? "-" : "hello"));
                if (valueChange == null)
                    return Fail(25, "no Value change event");
                values.Dispose();

                // The check box: the Toggle pattern.
                var box = window.FindFirstDescendant(cf => cf.ByAutomationId("103").And(cf.ByControlType(ControlType.CheckBox)));
                if (box == null)
                    return Fail(9, "no check box 103");
                var checkBox = box.AsCheckBox();
                var toggles = box.RegisterPropertyChangedEvent(TreeScope.Element,
                    (e, p, v) => Events.Enqueue("toggle=" + v), automation.PropertyLibrary.Toggle.ToggleState);
                Console.WriteLine("checked=" + checkBox.IsChecked);
                if (checkBox.IsChecked != false)
                    return Fail(10, "initially checked");
                checkBox.Toggle();
                var toggled = Retry.WhileFalse(() => checkBox.IsChecked == true, TimeSpan.FromSeconds(10),
                    TimeSpan.FromMilliseconds(50));
                Console.WriteLine("checked=" + checkBox.IsChecked);
                if (!toggled.Result)
                    return Fail(11, "not checked");
                // ToggleState_On (1), as FlaUI hands it over.
                var toggleChange = Event("toggle=");
                Console.WriteLine("togglechange=" + (toggleChange == "1" ? "On" : toggleChange ?? "-"));
                if (toggleChange != "1" && toggleChange != "On")
                    return Fail(26, "no ToggleState change event");
                toggles.Dispose();

                // The list view: its items as a ListBox's.
                var listElement = window.FindFirstDescendant(cf => cf.ByAutomationId("104"));
                if (listElement == null)
                    return Fail(14, "no list 104");
                var list = listElement.AsListBox();
                var items = list.Items;
                var texts = "";
                foreach (var item in items)
                    texts += (texts.Length > 0 ? "," : "") + item.Text;
                Console.WriteLine("items=" + items.Length + " " + texts);
                if (items.Length != 3 || texts != "alpha.txt,beta.txt,gamma.txt")
                    return Fail(15, "items");
                var selections = listElement.RegisterAutomationEvent(
                    automation.EventLibrary.SelectionItem.ElementSelectedEvent, TreeScope.Subtree,
                    (e, id) => Events.Enqueue("selected=" + Safe(() => e.Name)));
                items[1].Select();
                var selected = list.SelectedItem;
                Console.WriteLine("selected=" + (selected == null ? "-" : selected.Text) + " " +
                    items[1].IsSelected + " " + items[0].IsSelected);
                if (selected == null || selected.Text != "beta.txt" || !items[1].IsSelected || items[0].IsSelected)
                    return Fail(16, "selection");
                var selectedEvent = Event("selected=");
                Console.WriteLine("selectedevent=" + (selectedEvent ?? "-"));
                if (selectedEvent != "beta.txt")
                    return Fail(27, "no ElementSelected event");
                selections.Dispose();

                // The list view as a grid: rows, columns, the header's
                // items, cells.
                var grid = listElement.AsGrid();
                var headers = string.Join(",", grid.ColumnHeaders.Select(h => h.Name));
                var header = grid.Header;
                var headerNames = header == null ? "-" : string.Join(",", header.Columns.Select(c => c.Text));
                Console.WriteLine("grid=" + grid.RowCount + "x" + grid.ColumnCount + " " + grid.RowOrColumnMajor +
                    " headers=" + headers + " header=" + headerNames);
                if (grid.RowCount != 3 || grid.ColumnCount != 2 || headers != "Name,Size" || headerNames != "Name,Size")
                    return Fail(28, "grid");
                var cell = grid.Patterns.Grid.Pattern.GetItem(1, 1);
                var gridItem = cell.Patterns.GridItem.Pattern;
                var columnHeader = cell.Patterns.TableItem.Pattern.ColumnHeaderItems.Value;
                Console.WriteLine("cell=" + cell.Name + " " + gridItem.Row.Value + "," + gridItem.Column.Value + " " +
                    gridItem.ContainingGrid.Value.AutomationId + " " +
                    (columnHeader.Length == 1 ? columnHeader[0].Name : "-"));
                if (cell.Name != "22" || gridItem.Row.Value != 1 || gridItem.Column.Value != 1 ||
                    gridItem.ContainingGrid.Value.AutomationId != "104" || columnHeader.Length != 1 ||
                    columnHeader[0].Name != "Size")
                    return Fail(29, "cell");
                var row = grid.GetRowByIndex(2);
                var cells = string.Join(",", row.Cells.Select(c => c.Name));
                Console.WriteLine("row=" + cells);
                if (cells != "gamma.txt,333")
                    return Fail(30, "row");

                // The Window pattern (a WS_OVERLAPPEDWINDOW: it can be
                // maximized and minimized; not modal, not topmost).
                var pattern = window.Patterns.Window.Pattern;
                Console.WriteLine("window=" + pattern.CanMaximize.Value + " " + pattern.CanMinimize.Value + " " +
                    pattern.IsModal.Value + " " + pattern.IsTopmost.Value + " " + pattern.WindowVisualState.Value +
                    " modal=" + window.ModalWindows.Length);
                if (!pattern.CanMaximize.Value || !pattern.CanMinimize.Value || pattern.IsModal.Value ||
                    pattern.WindowVisualState.Value != WindowVisualState.Normal || window.ModalWindows.Length != 0)
                    return Fail(17, "window pattern");
                // The title bar (no name of its own, as Windows 11's proxy
                // gives it) and its buttons.
                var titleBar = window.TitleBar;
                var names = "";
                if (titleBar != null)
                    foreach (var c in titleBar.FindAllChildren())
                        names += (names.Length > 0 ? "," : "") + c.AutomationId;
                Console.WriteLine("titlebar=" + (titleBar == null ? "-" : titleBar.Name) + " " + names);
                if (titleBar == null || !string.IsNullOrEmpty(titleBar.Name) || titleBar.CloseButton == null ||
                    titleBar.MinimizeButton == null || titleBar.MaximizeButton == null)
                    return Fail(18, "title bar");
                // Window.Move: the Transform pattern.
                window.Move(100, 80);
                var moved = Retry.WhileFalse(() => window.BoundingRectangle.Left == 100 &&
                    window.BoundingRectangle.Top == 80, TimeSpan.FromSeconds(10), TimeSpan.FromMilliseconds(50));
                Console.WriteLine("moved=" + window.BoundingRectangle.Left + "," + window.BoundingRectangle.Top);
                if (!moved.Result)
                    return Fail(19, "not moved");
                // SetWindowVisualState: maximized, then back.
                pattern.SetWindowVisualState(WindowVisualState.Maximized);
                var maximized = Retry.WhileFalse(() => pattern.WindowVisualState.Value == WindowVisualState.Maximized,
                    TimeSpan.FromSeconds(10), TimeSpan.FromMilliseconds(50));
                Console.WriteLine("state=" + pattern.WindowVisualState.Value);
                if (!maximized.Result || titleBar.RestoreButton == null)
                    return Fail(20, "not maximized");
                pattern.SetWindowVisualState(WindowVisualState.Normal);
                var normal = Retry.WhileFalse(() => pattern.WindowVisualState.Value == WindowVisualState.Normal,
                    TimeSpan.FromSeconds(10), TimeSpan.FromMilliseconds(50));
                Console.WriteLine("state=" + pattern.WindowVisualState.Value + " " +
                    window.BoundingRectangle.Left + "," + window.BoundingRectangle.Top);
                if (!normal.Result || window.BoundingRectangle.Left != 100 || window.BoundingRectangle.Top != 80)
                    return Fail(21, "not restored");

                // Window.Close: the title bar's Close button; the program
                // ends.
                var closing = desktop.RegisterAutomationEvent(automation.EventLibrary.Window.WindowClosedEvent,
                    TreeScope.Subtree, (e, id) => Events.Enqueue("closed"));
                window.Close();
                var closed = Retry.WhileFalse(() => app.HasExited, TimeSpan.FromSeconds(10),
                    TimeSpan.FromMilliseconds(50));
                Console.WriteLine("closed=" + closed.Result);
                if (!closed.Result)
                    return Fail(12, "not closed");
                var closedEvent = Event("closed");
                Console.WriteLine("closedevent=" + (closedEvent != null));
                if (closedEvent == null)
                    return Fail(31, "no WindowClosed event");
                closing.Dispose();
                automation.UnregisterAllEvents();
            }
            finally
            {
                app.Dispose();
            }
        }
        Console.WriteLine("flaui_test: ok");
        return 0;
    }
}
