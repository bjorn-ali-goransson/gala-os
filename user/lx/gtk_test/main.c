/* gtk_test: the first GTK program on gala (ROADMAP LD step 3), a GTK 3
 * application as a distribution builds one: linked against Ubuntu
 * 24.04's libgtk-3.so.0 and started through glibc's ld.so, which loads
 * GTK's whole library closure (GLib, GIO, Pango, cairo, HarfBuzz,
 * fontconfig, FreeType, libwayland-client, libxkbcommon, ...). It opens a
 * window titled "gtk_test" holding a label and a button on the Wayland
 * display it is given (GDK_BACKEND=wayland), and says when the window
 * was first drawn, when the button was clicked, when the window was
 * asked to close and when it went.
 *
 * Who clicks: run as `./gtk_test --input` it waits for a real click on
 * its button and on its title bar's close button (gala's lx_gtk scenario
 * sends them through the desktop: weston's headless backend, where the
 * transcript is recorded, has no input devices). Run any other way (as
 * `cargo xtask lx-expect` runs it) it clicks its button itself once the
 * window has been drawn (gtk_button_clicked) and closes the window
 * itself (gtk_window_close), which GTK handles as the same signals as a
 * real click; the transcript is the same either way.
 *
 * No GTK headers: the few functions used are declared below from GTK's
 * and GLib's reference documentation (docs.gtk.org), so the program is
 * built with glibc's headers only (user/lx/build.sh's GLIBC_TESTS, with
 * GTK_LIBS) and links against the shared libraries by soname. It prints only what is
 * the same on any run: no addresses, sizes or times. */
#define _GNU_SOURCE
#include <stdio.h>
#include <string.h>

typedef struct _GtkWidget GtkWidget;
typedef int gboolean;
typedef void (*GCallback)(void);
typedef gboolean (*GSourceFunc)(void *data);

enum { GTK_WINDOW_TOPLEVEL = 0 };
enum { GTK_ORIENTATION_VERTICAL = 1 };
enum { G_CONNECT_AFTER = 1 };

extern void gtk_init(int *argc, char ***argv);
extern unsigned gtk_get_major_version(void);
extern unsigned gtk_get_minor_version(void);
extern void *gdk_display_get_default(void);
extern const char *g_type_name(unsigned long type);
extern GtkWidget *gtk_window_new(int type);
extern void gtk_window_set_title(GtkWidget *window, const char *title);
extern void gtk_window_set_default_size(GtkWidget *window, int width, int height);
extern void gtk_window_close(GtkWidget *window);
extern GtkWidget *gtk_box_new(int orientation, int spacing);
extern void gtk_box_pack_start(GtkWidget *box, GtkWidget *child, gboolean expand,
			       gboolean fill, unsigned padding);
extern void gtk_container_add(GtkWidget *container, GtkWidget *child);
extern void gtk_container_set_border_width(GtkWidget *container, unsigned width);
extern GtkWidget *gtk_label_new(const char *text);
extern GtkWidget *gtk_button_new_with_label(const char *label);
extern const char *gtk_button_get_label(GtkWidget *button);
extern void gtk_button_clicked(GtkWidget *button);
extern void gtk_widget_show_all(GtkWidget *widget);
extern void gtk_main(void);
extern void gtk_main_quit(void);
extern unsigned long g_signal_connect_data(void *instance, const char *signal,
					   GCallback handler, void *data,
					   void *destroy_data, int flags);
extern unsigned g_idle_add(GSourceFunc function, void *data);

/* G_OBJECT_TYPE_NAME: an instance starts with its class, a class with
 * its type. */
struct type_instance {
	struct {
		unsigned long type;
	} *klass;
};

static int self_driven = 1;
static int drawn;
static GtkWidget *window, *button;

static gboolean click_now(void *data)
{
	(void)data;
	gtk_button_clicked(button);
	return 0;
}

static gboolean close_now(void *data)
{
	(void)data;
	gtk_window_close(window);
	return 0;
}

/* "draw", after the window's own handler: it has been drawn. */
static gboolean on_draw(GtkWidget *widget, void *cr, void *data)
{
	(void)widget;
	(void)cr;
	(void)data;
	if (!drawn) {
		drawn = 1;
		printf("window drawn\n");
		if (self_driven)
			g_idle_add(click_now, NULL);
	}
	return 0;
}

static void on_clicked(GtkWidget *widget, void *data)
{
	(void)data;
	printf("button clicked: %s\n", gtk_button_get_label(widget));
	if (self_driven)
		g_idle_add(close_now, NULL);
}

static gboolean on_delete(GtkWidget *widget, void *event, void *data)
{
	(void)widget;
	(void)event;
	(void)data;
	printf("close requested\n");
	return 0; /* let it go */
}

static void on_destroy(GtkWidget *widget, void *data)
{
	(void)widget;
	(void)data;
	printf("window destroyed\n");
	gtk_main_quit();
}

int main(int argc, char **argv)
{
	/* One line at a time: gala's scenario waits for each. */
	setvbuf(stdout, NULL, _IOLBF, 0);
	for (int i = 1; i < argc; i++)
		if (strcmp(argv[i], "--input") == 0)
			self_driven = 0;
	gtk_init(&argc, &argv);
	printf("gtk version = %u.%u\n", gtk_get_major_version(), gtk_get_minor_version());
	struct type_instance *display = gdk_display_get_default();
	printf("display = %s\n", display ? g_type_name(display->klass->type) : "none");
	if (!display)
		return 1;

	window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
	gtk_window_set_title(window, "gtk_test");
	gtk_window_set_default_size(window, 320, 200);
	gtk_container_set_border_width(window, 24);
	GtkWidget *box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 16);
	GtkWidget *label = gtk_label_new("Hello from GTK on gala");
	button = gtk_button_new_with_label("Click me");
	gtk_box_pack_start(box, label, 1, 1, 0);
	gtk_box_pack_start(box, button, 0, 0, 0);
	gtk_container_add(window, box);
	g_signal_connect_data(window, "draw", (GCallback)on_draw, NULL, NULL, G_CONNECT_AFTER);
	g_signal_connect_data(window, "delete-event", (GCallback)on_delete, NULL, NULL, 0);
	g_signal_connect_data(window, "destroy", (GCallback)on_destroy, NULL, NULL, 0);
	g_signal_connect_data(button, "clicked", (GCallback)on_clicked, NULL, NULL, 0);
	gtk_widget_show_all(window);
	gtk_main();
	printf("gtk_test: ok\n");
	return 0;
}
