import android.accessibilityservice.AccessibilityServiceInfo;
import android.app.UiAutomation;
import android.os.HandlerThread;
import android.os.SystemClock;
import android.view.accessibility.AccessibilityNodeInfo;
import android.graphics.Rect;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.lang.reflect.Constructor;
import java.lang.reflect.Method;

/**
 * Persistent accessibility dump server for ADB. Runs under app_process with
 * shell permissions, connects a UiAutomation once, then serves repeated
 * hierarchy dumps over stdin/stdout: read the line "dump", write the tree
 * as uiautomator-compatible XML terminated by "###END###".
 *
 * This avoids the ~2s per-call cost of `uiautomator dump`, which spawns a
 * fresh JVM, connects UiAutomation, dumps once, and exits.
 */
public final class AxDump {
    private AxDump() {}

    /**
     * Hidden {@code AccessibilityNodeInfo.getSourceNodeId()}: a per-window
     * stable identity for the underlying view (virtual descendant id packed
     * with the accessibility view id). Survives text/label changes and moves,
     * so hosts can tell "same control, new state" from "replaced control".
     */
    private static Method sourceNodeId;

    static {
        try {
            sourceNodeId = AccessibilityNodeInfo.class.getDeclaredMethod("getSourceNodeId");
            sourceNodeId.setAccessible(true);
        } catch (Exception ignored) {
            sourceNodeId = null;
        }
    }

    public static void main(String[] args) throws Exception {
        HandlerThread thread = new HandlerThread("axdump");
        thread.start();

        UiAutomation automation = connect(thread);
        AccessibilityServiceInfo info = automation.getServiceInfo();
        if (info != null) {
            info.flags |= AccessibilityServiceInfo.FLAG_INCLUDE_NOT_IMPORTANT_VIEWS
                | AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS;
            automation.setServiceInfo(info);
        }

        System.out.println("axdump ready");
        System.out.flush();

        BufferedReader stdin = new BufferedReader(new InputStreamReader(System.in));
        String line;
        while ((line = stdin.readLine()) != null) {
            line = line.trim();
            if (line.equals("quit")) {
                break;
            }
            if (!line.equals("dump")) {
                continue;
            }
            long started = SystemClock.uptimeMillis();
            AccessibilityNodeInfo root = automation.getRootInActiveWindow();
            StringBuilder out = new StringBuilder(16 * 1024);
            if (root == null) {
                out.append("error: no active window root\n");
            } else {
                out.append("<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>");
                out.append("<hierarchy rotation=\"0\">");
                emit(root, 0, out);
                out.append("</hierarchy>\n");
            }
            out.append("###END### ").append(SystemClock.uptimeMillis() - started).append("ms\n");
            System.out.print(out);
            System.out.flush();
        }
        // Unregister the UiTestAutomationService before exiting: a bare
        // System.exit can leave the registration alive in the accessibility
        // manager long enough that the next AxDump start dies with
        // "UiAutomationService ... already registered!".
        try {
            Method disconnect = UiAutomation.class.getDeclaredMethod("disconnect");
            disconnect.setAccessible(true);
            disconnect.invoke(automation);
        } catch (Exception ignored) {
        }
        System.exit(0);
    }

    private static UiAutomation connect(HandlerThread thread) throws Exception {
        // UiAutomationConnection and IUiAutomationConnection are hidden APIs:
        // reach them purely reflectively so android.jar can compile this.
        Class<?> connectionClass = Class.forName("android.app.UiAutomationConnection");
        Object connection = connectionClass.getDeclaredConstructor().newInstance();
        Class<?> interfaceClass = Class.forName("android.app.IUiAutomationConnection");
        Constructor<UiAutomation> constructor = UiAutomation.class.getDeclaredConstructor(
            android.os.Looper.class, interfaceClass);
        constructor.setAccessible(true);
        UiAutomation automation = constructor.newInstance(thread.getLooper(), connection);
        Method connect;
        try {
            connect = UiAutomation.class.getDeclaredMethod("connect", int.class);
            connect.setAccessible(true);
            connect.invoke(automation, 0);
        } catch (NoSuchMethodException e) {
            connect = UiAutomation.class.getDeclaredMethod("connect");
            connect.setAccessible(true);
            connect.invoke(automation);
        }
        return automation;
    }

    private static void emit(AccessibilityNodeInfo node, int index, StringBuilder out) {
        if (node == null || !node.isVisibleToUser()) {
            // uiautomator dump prunes invisible subtrees (unrealized/off-screen
            // list rows report clamped, inverted bounds); match that.
            return;
        }
        Rect bounds = new Rect();
        node.getBoundsInScreen(bounds);
        out.append("<node index=\"").append(index).append('"');
        attr(out, "text", node.getText());
        attr(out, "resource-id", node.getViewIdResourceName());
        attr(out, "class", node.getClassName());
        attr(out, "package", node.getPackageName());
        attr(out, "content-desc", node.getContentDescription());
        flag(out, "checkable", node.isCheckable());
        flag(out, "checked", node.isChecked());
        flag(out, "clickable", node.isClickable());
        flag(out, "enabled", node.isEnabled());
        flag(out, "focusable", node.isFocusable());
        flag(out, "focused", node.isFocused());
        flag(out, "scrollable", node.isScrollable());
        flag(out, "long-clickable", node.isLongClickable());
        flag(out, "password", node.isPassword());
        flag(out, "selected", node.isSelected());
        if (sourceNodeId != null) {
            try {
                Object id = sourceNodeId.invoke(node);
                if (id instanceof Long) {
                    out.append(" ax-node-id=\"").append(((Long) id).longValue()).append('"');
                }
            } catch (Exception ignored) {
            }
        }
        out.append(" bounds=\"")
            .append('[').append(bounds.left).append(',').append(bounds.top).append(']')
            .append('[').append(bounds.right).append(',').append(bounds.bottom).append(']')
            .append('"');
        int count = node.getChildCount();
        if (count == 0) {
            out.append(" />");
            return;
        }
        out.append('>');
        for (int i = 0; i < count; i++) {
            emit(node.getChild(i), i, out);
        }
        out.append("</node>");
    }

    private static void attr(StringBuilder out, String name, CharSequence value) {
        out.append(' ').append(name).append("=\"");
        if (value != null) {
            String text = value.toString();
            for (int i = 0; i < text.length(); i++) {
                char c = text.charAt(i);
                switch (c) {
                    case '&': out.append("&amp;"); break;
                    case '<': out.append("&lt;"); break;
                    case '>': out.append("&gt;"); break;
                    case '"': out.append("&quot;"); break;
                    case '\'': out.append("&apos;"); break;
                    default:
                        if (c < 0x20 && c != '\t') {
                            out.append(' ');
                        } else {
                            out.append(c);
                        }
                }
            }
        }
        out.append('"');
    }

    private static void flag(StringBuilder out, String name, boolean value) {
        out.append(' ').append(name).append("=\"").append(value).append('"');
    }
}
