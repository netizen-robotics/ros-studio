# Inspect and interact with ROS using Tools

Use Tools to browse the ROS graph, inspect data, and prepare requests.

## Browse the ROS graph

After [connecting](../getting-started.md), choose **Topics**, **Services**, **Actions**, **Nodes**, or **Parameters**. Search for an interface and open its task. Browsing and opening drafts do not send commands or start topic reception.

Topic tasks include message, plot, and image inspection alongside publishing, endpoint, and definition information. Node tasks show their related interfaces. Independent inspection views are also available through **Dashboard**.

## Inspect messages, plots, and images

Configure the exact topic, type, and quality of service (QoS), then explicitly enable reception. In Messages, inspect structured values. To plot data, select a supported numeric field and open the plot. Use Image for supported raw image encodings.

Review sample statistics and dropped-sample information when reception is busy. Plot **Pause** and **Clear**, and Image **Pause**, affect local inspection. They do not claim to stop a publisher.

The **View** selector chooses an existing inspection view. Sample counts appear beside the content; nonzero queue drops remain visible, with full counters under **Reception diagnostics**. In **Dashboard**, use **Receive data** in view settings to start or stop reception.

Changing the selected interface leaves active work attached to its original source. Review source changes and apply them explicitly.

## Send an explicit request

When connected, valid drafts enable the named operation buttons. Review the target and draft, then deliberately choose Publish, Call service, Send goal, or the parameter write action. Opening a task, loading a schema, and editing fields never execute the operation.

Typed forms and YAML create validated drafts. Imported YAML does not execute operations. Parameter tasks distinguish current values, edited drafts, and server readback; a submitted write is not proof that every parameter was accepted.

Action tasks track goals created by the session. A cancellation acknowledgement is separate from the action's terminal result. Stopping local waiting or tracking does not prove that remote execution stopped.

## Stop or reconnect

Stop the relevant operation before replacing its configuration. Disconnect ends local monitoring. Reconnect and workspace restore leave readers inactive; commands do not replay.

For discovery, QoS, and diagnostic problems, see [troubleshooting](../troubleshooting.md).

## Interface schemas

Common Jazzy `nav2_msgs` action interfaces and their native type-support dependencies are bundled. This supplies schemas, not navigation servers or dedicated navigation controls. ROS discovery advertises type names; it does not transfer interface libraries. For other packages, import a matching ROS-distribution and platform interface bundle from the Definition task. A schema-loading error offers **Choose interface bundle…** for recovery.
