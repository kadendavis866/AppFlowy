import 'package:appflowy/mobile/presentation/inline_actions/mobile_inline_actions_menu.dart';
import 'package:appflowy/plugins/inline_actions/inline_actions_menu.dart';
import 'package:appflowy/plugins/inline_actions/inline_actions_result.dart';
import 'package:appflowy/plugins/inline_actions/inline_actions_service.dart';
import 'package:appflowy_editor/appflowy_editor.dart';
import 'package:universal_platform/universal_platform.dart';

const inlineActionCharacter = '@';

CharacterShortcutEvent inlineActionsCommand(
  InlineActionsService inlineActionsService, {
  InlineActionsMenuStyle style = const InlineActionsMenuStyle.light(),
}) =>
    CharacterShortcutEvent(
      key: 'Opens Inline Actions Menu',
      character: inlineActionCharacter,
      handler: (editorState) => inlineActionsCommandHandler(
        editorState,
        inlineActionsService,
        style,
      ),
    );

InlineActionsMenuService? selectionMenuService;

Future<bool> inlineActionsCommandHandler(
  EditorState editorState,
  InlineActionsService service,
  InlineActionsMenuStyle style,
) async {
  final selection = editorState.selection;
  if (selection == null) {
    return false;
  }

  if (!selection.isCollapsed) {
    await editorState.deleteSelection(selection);
  }

  await editorState.insertTextAtPosition(
    inlineActionCharacter,
    position: selection.start,
  );

  final List<InlineActionsResult> initialResults = [];
  for (final handler in service.handlers) {
    final group = await handler.search(null);

    if (group.results.isNotEmpty) {
      initialResults.add(group);
    }
  }

  if (service.context != null) {
    keepEditorFocusNotifier.increase();
    selectionMenuService?.dismiss();
    selectionMenuService = UniversalPlatform.isMobile
        ? MobileInlineActionsMenu(
            context: service.context!,
            editorState: editorState,
            service: service,
            initialResults: initialResults,
            style: style,
          )
        : InlineActionsMenu(
            context: service.context!,
            editorState: editorState,
            service: service,
            initialResults: initialResults,
            style: style,
          );

    // disable the keyboard service
    editorState.service.keyboardService?.disable();

    await selectionMenuService?.show();

    // enable the keyboard service
    editorState.service.keyboardService?.enable();
  }

  return true;
}
