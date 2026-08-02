/*
 * The resident accessibility bridge: rung 2 on Android.
 *
 * Why a service at all, rather than `uiautomator dump` over adb? Because
 * `uiautomator dump` fails outright with "ERROR: could not get idle state" on
 * any screen that is animating — which is exactly the screen an agent wants to
 * look at after acting. It is a tool built for a human waiting for things to
 * stop. A resident AccessibilityService has no such requirement: it holds the
 * tree continuously, answers immediately, and *pushes* a notification when the
 * content changes, which is the settle signal rung 2 needs.
 *
 * The transport is a loopback socket inside the container, reached from the host
 * through `adb forward`. Deliberately not a content provider or a broadcast:
 * both are request/response only, and the whole point is the outbound event.
 *
 * Node identity is generational. A tree walk assigns "<generation>:<index>" to
 * every node and remembers that generation's nodes; a request naming an older
 * generation is refused rather than resolved against whatever now sits at that
 * index. AccessibilityNodeInfo has no identity that survives a refresh, so the
 * alternative is acting on a node that has silently become a different one —
 * the precise failure the anchor contract exists to prevent.
 */
package dev.artist.bridge;

import android.accessibilityservice.AccessibilityService;
import android.accessibilityservice.AccessibilityServiceInfo;
import android.graphics.Rect;
import android.os.Bundle;
import android.util.Log;
import android.view.accessibility.AccessibilityEvent;
import android.view.accessibility.AccessibilityNodeInfo;
import android.view.accessibility.AccessibilityWindowInfo;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.PrintWriter;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicReference;

public final class BridgeService extends AccessibilityService {
    private static final String TAG = "artist-bridge";

    /** Loopback inside the container; the host reaches it via `adb forward`. */
    private static final int PORT = 8722;

    /**
     * A tree deeper than this is a runaway, not a UI.
     *
     * Some WebViews expose recursive structures through the accessibility API,
     * and a walk with no bound hangs the service rather than the app.
     */
    private static final int MAX_DEPTH = 60;

    /** Above this the observation is unusable anyway, and the walk is costing more than it returns. */
    private static final int MAX_NODES = 4000;

    private final AtomicReference<PrintWriter> client = new AtomicReference<>(null);
    private final Map<String, AccessibilityNodeInfo> nodes = new HashMap<>();
    private int generation = 0;
    private Thread server;

    @Override
    protected void onServiceConnected() {
        AccessibilityServiceInfo info = new AccessibilityServiceInfo();
        info.eventTypes = AccessibilityEvent.TYPES_ALL_MASK;
        info.feedbackType = AccessibilityServiceInfo.FEEDBACK_GENERIC;
        // Zero, not the default 100ms: the notification *is* the settle signal,
        // and coalescing it introduces a delay the harness would have to guess
        // at from the outside.
        info.notificationTimeout = 0;
        info.flags = AccessibilityServiceInfo.DEFAULT
                // Without this, viewIdResourceName is null for every node and
                // the most stable name a node has is unavailable.
                | AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS
                // Dialogs, IMEs and system windows are separate windows; without
                // this only the active one is visible and a permission prompt is
                // invisible to the agent that triggered it.
                | AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS;
        setServiceInfo(info);
        startServer();
    }

    private void startServer() {
        if (server != null) {
            return;
        }
        server = new Thread(() -> {
            try (ServerSocket socket = new ServerSocket(PORT, 1, InetAddress.getByName("127.0.0.1"))) {
                socket.setReuseAddress(true);
                while (!Thread.currentThread().isInterrupted()) {
                    try (Socket connection = socket.accept()) {
                        serve(connection);
                    } catch (IOException error) {
                        Log.w(TAG, "connection ended: " + error);
                    } finally {
                        client.set(null);
                    }
                }
            } catch (IOException error) {
                Log.e(TAG, "could not listen on " + PORT, error);
            }
        }, "artist-bridge");
        server.setDaemon(true);
        server.start();
    }

    private void serve(Socket connection) throws IOException {
        connection.setTcpNoDelay(true);
        BufferedReader reader = new BufferedReader(
                new InputStreamReader(connection.getInputStream(), StandardCharsets.UTF_8));
        PrintWriter writer = new PrintWriter(
                new java.io.OutputStreamWriter(connection.getOutputStream(), StandardCharsets.UTF_8), true);
        client.set(writer);

        String line;
        while ((line = reader.readLine()) != null) {
            if (line.trim().isEmpty()) {
                continue;
            }
            JSONObject reply;
            long id = -1;
            try {
                JSONObject request = new JSONObject(line);
                id = request.optLong("id", -1);
                reply = dispatch(request);
            } catch (Throwable error) {
                // Every failure is reported to the caller rather than logged and
                // dropped. A request that never gets an answer is indelible: the
                // harness waits out its timeout with nothing to say about why.
                reply = new JSONObject();
                try {
                    reply.put("error", String.valueOf(error));
                } catch (Throwable ignored) {
                }
            }
            try {
                reply.put("id", id);
            } catch (Throwable ignored) {
            }
            writer.println(reply.toString());
        }
    }

    private JSONObject dispatch(JSONObject request) throws Exception {
        String op = request.optString("op", "");
        JSONObject reply = new JSONObject();
        switch (op) {
            case "ping":
                reply.put("ok", true);
                reply.put("service", "artist-bridge");
                return reply;
            case "tree":
                return tree();
            case "act":
                return act(request.getString("node"), request.getString("action"));
            case "setText":
                return setText(request.getString("node"), request.getString("text"));
            default:
                reply.put("error", "unknown op " + op);
                return reply;
        }
    }

    /** Every node on every interactive window, with a fresh generation of ids. */
    private synchronized JSONObject tree() throws Exception {
        for (AccessibilityNodeInfo held : nodes.values()) {
            held.recycle();
        }
        nodes.clear();
        generation++;

        JSONArray out = new JSONArray();
        int[] counter = new int[] {0};

        List<AccessibilityWindowInfo> windows = getWindows();
        if (windows == null || windows.isEmpty()) {
            AccessibilityNodeInfo root = getRootInActiveWindow();
            if (root != null) {
                walk(root, -1, 0, out, counter);
            }
        } else {
            for (AccessibilityWindowInfo window : windows) {
                AccessibilityNodeInfo root = window.getRoot();
                if (root != null) {
                    walk(root, -1, 0, out, counter);
                }
            }
        }

        JSONObject reply = new JSONObject();
        reply.put("generation", generation);
        reply.put("nodes", out);
        reply.put("truncated", counter[0] >= MAX_NODES);
        return reply;
    }

    private void walk(AccessibilityNodeInfo node, int parent, int depth, JSONArray out, int[] counter)
            throws Exception {
        if (node == null || depth > MAX_DEPTH || counter[0] >= MAX_NODES) {
            return;
        }
        int index = counter[0]++;
        String id = generation + ":" + index;
        // Held, not recycled: an action arriving later needs this exact object,
        // and a recycled AccessibilityNodeInfo throws when touched.
        nodes.put(id, node);

        Rect bounds = new Rect();
        node.getBoundsInScreen(bounds);

        JSONObject json = new JSONObject();
        json.put("id", id);
        json.put("parent", parent < 0 ? JSONObject.NULL : generation + ":" + parent);
        json.put("class", text(node.getClassName()));
        json.put("package", text(node.getPackageName()));
        json.put("text", text(node.getText()));
        json.put("desc", text(node.getContentDescription()));
        json.put("resId", text(node.getViewIdResourceName()));
        json.put("hint", text(node.getHintText()));
        JSONArray rect = new JSONArray();
        rect.put(bounds.left);
        rect.put(bounds.top);
        rect.put(bounds.right);
        rect.put(bounds.bottom);
        json.put("bounds", rect);
        json.put("clickable", node.isClickable());
        json.put("longClickable", node.isLongClickable());
        json.put("scrollable", node.isScrollable());
        json.put("editable", node.isEditable());
        json.put("checkable", node.isCheckable());
        json.put("checked", node.isChecked());
        json.put("enabled", node.isEnabled());
        json.put("focused", node.isFocused());
        json.put("selected", node.isSelected());
        json.put("visible", node.isVisibleToUser());
        out.put(json);

        for (int child = 0; child < node.getChildCount(); child++) {
            walk(node.getChild(child), index, depth + 1, out, counter);
        }
    }

    private synchronized JSONObject act(String id, String action) throws Exception {
        JSONObject reply = new JSONObject();
        AccessibilityNodeInfo node = resolve(id, reply);
        if (node == null) {
            return reply;
        }

        int code;
        switch (action) {
            case "click":
                code = AccessibilityNodeInfo.ACTION_CLICK;
                break;
            case "longClick":
                code = AccessibilityNodeInfo.ACTION_LONG_CLICK;
                break;
            case "focus":
                code = AccessibilityNodeInfo.ACTION_FOCUS;
                break;
            case "scrollForward":
                code = AccessibilityNodeInfo.ACTION_SCROLL_FORWARD;
                break;
            case "scrollBackward":
                code = AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD;
                break;
            default:
                reply.put("error", "unknown action " + action);
                return reply;
        }

        boolean performed = node.performAction(code);
        reply.put("ok", performed);
        if (!performed) {
            // A node can be visible, enabled and still refuse an action — most
            // often because the clickable node is an ancestor. Saying so is what
            // lets the harness retry against the parent instead of reporting a
            // successful click that did nothing.
            reply.put("error", "the node did not accept " + action);
        }
        return reply;
    }

    private synchronized JSONObject setText(String id, String value) throws Exception {
        JSONObject reply = new JSONObject();
        AccessibilityNodeInfo node = resolve(id, reply);
        if (node == null) {
            return reply;
        }
        Bundle arguments = new Bundle();
        arguments.putCharSequence(
                AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, value);
        boolean performed = node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, arguments);
        reply.put("ok", performed);
        if (!performed) {
            reply.put("error", "the node did not accept text");
        }
        return reply;
    }

    private AccessibilityNodeInfo resolve(String id, JSONObject reply) throws Exception {
        AccessibilityNodeInfo node = nodes.get(id);
        if (node != null) {
            return node;
        }
        String prefix = generation + ":";
        if (!id.startsWith(prefix)) {
            reply.put("error", "stale node " + id + " — the tree has changed, observe it again");
            reply.put("stale", true);
        } else {
            reply.put("error", "no node " + id);
        }
        return null;
    }

    @Override
    public void onAccessibilityEvent(AccessibilityEvent event) {
        PrintWriter writer = client.get();
        if (writer == null || event == null) {
            return;
        }
        int type = event.getEventType();
        if (type != AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED
                && type != AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED
                && type != AccessibilityEvent.TYPE_WINDOWS_CHANGED) {
            return;
        }
        try {
            JSONObject notice = new JSONObject();
            notice.put("event", type == AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED
                    ? "content" : "window");
            notice.put("package", text(event.getPackageName()));
            notice.put("at", System.currentTimeMillis());
            writer.println(notice.toString());
        } catch (Throwable error) {
            Log.w(TAG, "could not push an event: " + error);
        }
    }

    @Override
    public void onInterrupt() {
    }

    private static String text(CharSequence value) {
        return value == null ? "" : value.toString();
    }
}
