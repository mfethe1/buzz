import 'package:integration_test/integration_test.dart';

import '../test/features/work/work_page_test.dart' as recovery;

// Run the same production WorkPage flows through a native Flutter engine.
// The owned HTTP responses remain isolated from the user's community.
void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  recovery.main();
}
